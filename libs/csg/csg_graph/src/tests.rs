use crate::*;
use std::collections::HashMap;

/// Edges used by other than exactly two triangles (after welding).
fn open_edges(m: &TriMesh) -> usize {
    let mut m = m.clone();
    m.weld_vertices(1e-7);
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    count.values().filter(|c| **c != 2).count()
}

fn rect(w: f64, h: f64) -> Sketch {
    Sketch::new(vec![Shape::Rect { center: [0.0, 0.0], size: [w, h], corner: 0.0, chamfer: 0.0 }])
}

#[test]
fn hull_of_a_shifted_drafted_box_has_prismatoid_volume() {
    use makepad_csg_mesh::validate::{pieces, validate_mesh};

    // The square-root formula applies to similar rectangles (top depth 4.5).
    // At depth 4 the exact prismatoid formula uses the middle section's area.
    for top_depth in [4.0, 4.5] {
        let mut points = Vec::new();
        for (width, depth, z, shift) in [(4.0, 3.0, -2.5, 0.0), (6.0, top_depth, 2.5, 0.5)] {
            for x in [-0.5, 0.5] {
                for y in [-0.5, 0.5] {
                    points.push([width * x + shift, depth * y, z]);
                }
            }
        }
        let mut g = Graph::new();
        let id = g.push(Op::Hull { points }).unwrap();
        let mut e = Evaluator::new();
        let mesh = e.mesh(&g, id, None).expect("a drafted box has a hull");
        let (a1, a2) = (12.0_f64, 6.0 * top_depth);
        let expected = if top_depth == 4.5 {
            5.0 / 3.0 * (a1 + a2 + (a1 * a2).sqrt())
        } else {
            let middle = 5.0 * (3.0 + top_depth) * 0.5;
            5.0 / 6.0 * (a1 + 4.0 * middle + a2)
        };
        let volume = makepad_csg_cad::signed_volume(&mesh);
        assert!((volume - expected).abs() < expected * 1e-4, "drafted hull volume: {volume} expected {expected}");
        let report = validate_mesh(&mesh);
        assert!(report.is_closed, "drafted hull is closed");
        assert!(report.is_manifold, "drafted hull is manifold");
        assert!(report.is_consistently_oriented, "drafted hull winding is consistent");
        assert!(report.near_duplicate_pairs == 0 && report.coincident_pairs == 0, "drafted hull: no near-duplicate or coincident vertex pairs: {report:?}");
        assert_eq!(pieces(&mesh), 1, "drafted hull is one piece");
        e.draft = 2.0;
        e.begin();
        assert!(Arc::ptr_eq(&mesh, &e.mesh(&g, id, None).unwrap()), "exact hull facets do not depend on draft");
    }
}

#[test]
fn hull_drops_cube_interior_and_coplanar_points_without_degenerate_faces() {
    use makepad_csg_math::dvec3;
    use makepad_csg_mesh::validate::validate_mesh;

    let cube = makepad_csg_primitives::cube(dvec3(2.0, 2.0, 2.0), true);
    let mut points: Vec<P3> = cube.vertices.iter().map(|p| [p.x, p.y, p.z]).collect();
    points.push([0.0; 3]);
    for axis in 0..3 {
        for a in [-1.0, 1.0] {
            for b in [-1.0, 1.0] {
                let mut p = [0.0; 3];
                p[(axis + 1) % 3] = a;
                p[(axis + 2) % 3] = b;
                points.push(p);
            }
        }
    }
    points.extend_from_within(..8);
    points.push([-0.0, 0.0, 0.0]);
    // Also grow through coplanar edge points before inserting the remaining corners.
    for _ in 0..2 {
        let mut g = Graph::new();
        let id = g.push(Op::Hull { points: points.clone() }).unwrap();
        let mesh = Evaluator::new().mesh(&g, id, None).expect("cube points have a hull");
        assert!((makepad_csg_cad::signed_volume(&mesh) - 8.0).abs() < 1e-9, "cube hull keeps the cube volume");
        let report = validate_mesh(&mesh);
        assert!(report.is_closed, "cube hull is closed");
        assert!(report.is_manifold, "cube hull is manifold");
        assert!(report.is_consistently_oriented, "cube hull winding is consistent");
        assert_eq!(report.degenerate_triangles, 0, "cube hull has no degenerate triangles");
        assert!(report.near_duplicate_pairs == 0 && report.coincident_pairs == 0, "cube hull: no near-duplicate or coincident vertex pairs: {report:?}");
        assert!(mesh.corner_normals.is_empty(), "hull faces use flat normals");
        assert!(!mesh.vertices.contains(&dvec3(0.0, 0.0, 0.0)), "interior points are not hull vertices");
        for (i, p) in mesh.vertices.iter().enumerate() {
            assert!(mesh.triangles.iter().any(|t| t.contains(&(i as u32))), "hull vertex {i} is used");
            assert!(!mesh.vertices[..i].contains(p), "hull vertex {i} is unique");
        }
        let repeated = Evaluator::new().mesh(&g, id, None).unwrap();
        assert_eq!(mesh.vertices, repeated.vertices, "hull vertex order is deterministic");
        assert_eq!(mesh.triangles, repeated.triangles, "hull triangulation is deterministic");
        points.rotate_left(10);
    }
}

#[test]
fn hull_of_sphere_points_is_closed_and_approximates_its_volume() {
    use makepad_csg_mesh::validate::validate_mesh;

    let angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let points = (0..200).map(|i| {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / 200.0;
        let r = (1.0 - z * z).sqrt();
        let theta = angle * i as f64;
        [r * theta.cos(), r * theta.sin(), z]
    }).collect();
    let mut g = Graph::new();
    let id = g.push(Op::Hull { points }).unwrap();
    let mesh = Evaluator::new().mesh(&g, id, None).expect("sphere points have a hull");
    let report = validate_mesh(&mesh);
    assert!(report.is_closed, "sphere hull is closed");
    assert!(report.is_manifold, "sphere hull is manifold");
    assert!(report.is_consistently_oriented, "sphere hull winding is consistent");
    assert_eq!(report.degenerate_triangles, 0, "sphere hull has no degenerate triangles");
    assert!(report.near_duplicate_pairs == 0 && report.coincident_pairs == 0, "sphere hull: no near-duplicate or coincident vertex pairs: {report:?}");
    let volume = makepad_csg_cad::signed_volume(&mesh);
    let sphere = 4.0 / 3.0 * std::f64::consts::PI;
    assert!(volume < sphere, "inscribed hull volume is below the sphere: {volume}");
    assert!(volume > 0.97 * sphere, "sphere hull retains over 97 percent of volume: {volume}");
}

/// A grid turned off the axes puts many points almost on each face's plane and edges: a point
/// almost in line with a horizon edge once left a sliver of no area, its normal rounding to
/// nothing. The hull is closed, with no flat face, and holds the grid's cube.
#[test]
fn hull_of_a_turned_grid_has_no_flat_faces() {
    use makepad_csg_mesh::validate::validate_mesh;

    let (s, c) = 0.5f64.sin_cos();
    let (s2, c2) = 0.2f64.sin_cos();
    let points = (0..1331).map(|i| {
        let p = [(i % 11) as f64, (i / 11 % 11) as f64, (i / 121) as f64];
        let q = [c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]];
        [q[0], c2 * q[1] - s2 * q[2], s2 * q[1] + c2 * q[2]]
    }).collect();
    let mut g = Graph::new();
    let id = g.push(Op::Hull { points }).unwrap();
    let mesh = Evaluator::new().mesh(&g, id, None).expect("a turned grid has a hull");
    let report = validate_mesh(&mesh);
    assert!(report.is_closed && report.is_manifold && report.is_consistently_oriented, "turned grid hull: closed, manifold, one winding: {report:?}");
    assert_eq!(report.degenerate_triangles, 0, "turned grid hull has no flat faces");
    let volume = makepad_csg_cad::signed_volume(&mesh);
    assert!((volume - 1000.0).abs() < 1e-9, "turned grid hull holds the grid's cube: {volume}");
}

#[test]
fn hull_requires_non_coplanar_points_and_uses_outward_predicates() {
    use makepad_csg_math::{robust::orient3d, Vec3d};

    assert!(orient3d(Vec3d::ZERO, Vec3d::X, Vec3d::Y, Vec3d::Z) > 0.0, "orient3d is positive on the CCW normal side");
    assert!(orient3d(Vec3d::ZERO, Vec3d::Y, Vec3d::X, Vec3d::Z) < 0.0, "reversing winding reverses orient3d sign");
    assert_eq!(orient3d(Vec3d::ZERO, Vec3d::X, Vec3d::Y, Vec3d::X + Vec3d::Y), 0.0, "coplanar points have zero orientation");
    let tetrahedron = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut g = Graph::new();
    let id = g.push(Op::Hull { points: tetrahedron.clone() }).unwrap();
    let mesh = Evaluator::new().mesh(&g, id, None).unwrap();
    assert!((makepad_csg_cad::signed_volume(&mesh) - 1.0 / 6.0).abs() < 1e-12, "seed tetrahedron is outward oriented");
    let mut invalid: Vec<_> = (0..4).map(|n| tetrahedron[..n].to_vec()).collect();
    invalid.extend([
        vec![[0.0; 3]; 4],
        vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]],
        vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
    ]);
    for points in invalid {
        let id = g.push(Op::Hull { points }).unwrap();
        let failure = Evaluator::new().mesh(&g, id, None).expect_err("degenerate point sets cannot form a hull");
        assert_eq!(failure.op, "hull", "invalid hull errors identify the operation");
        assert_eq!(failure.message, "a hull needs four points that are not in one plane", "invalid hull errors explain the geometric requirement");
    }
}

#[test]
fn hull_keys_include_every_point_coordinate() {
    let points = vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut g = Graph::new();
    let a = g.push(Op::Hull { points: points.clone() }).unwrap();
    let b = g.push(Op::Hull { points: points.clone() }).unwrap();
    assert_eq!(g.node(a).key, g.node(b).key, "identical hull operations have identical keys");
    assert!(g.node(a).op.inputs().is_empty(), "a hull has no graph inputs");
    for i in 0..points.len() {
        for axis in 0..3 {
            let mut changed = points.clone();
            changed[i][axis] += 0.25;
            let id = g.push(Op::Hull { points: changed }).unwrap();
            assert_ne!(g.node(a).key, g.node(id).key, "moving point {i} on axis {axis} changes the hull key");
        }
    }
}

fn square_loft_ring(z: f64, half_width: f64) -> Ring {
    Ring {
        origin: [0.0, 0.0, z], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0],
        points: vec![[-half_width, -half_width], [half_width, -half_width], [half_width, half_width], [-half_width, half_width]],
        crease: false,
    }
}

#[test]
fn ring_loft_with_a_tilted_top_has_cylinder_volume() {
    use makepad_csg_mesh::validate::{pieces, validate_mesh};

    let points: Vec<_> = (0..64).map(|i| {
        let angle = std::f64::consts::TAU * i as f64 / 64.0;
        [3.0 * angle.cos(), 3.0 * angle.sin()]
    }).collect();
    let bottom = Ring { origin: [0.0; 3], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], points, crease: false };
    let (s, c) = 20.0_f64.to_radians().sin_cos();
    let mut top = bottom.clone();
    top.origin[2] = 8.0;
    top.v = [0.0, c, s];
    for p in &mut top.points {
        p[1] /= c;
    }
    let mut g = Graph::new();
    let id = g.push(Op::RingLoft { rings: vec![bottom, top], prism: false }).unwrap();
    let mesh = Evaluator::new().mesh(&g, id, None).expect("a tilted ring loft evaluates");
    let expected = std::f64::consts::PI * 9.0 * 8.0;
    let volume = makepad_csg_cad::signed_volume(&mesh);
    assert!((volume - expected).abs() < expected * 0.004, "tilted ring loft cylinder volume: {volume} expected {expected}");
    let report = validate_mesh(&mesh);
    assert!(report.is_closed, "tilted ring loft is closed");
    assert!(report.is_manifold, "tilted ring loft is manifold");
    assert!(report.is_consistently_oriented, "tilted ring loft winding is consistent");
    assert_eq!(report.degenerate_triangles, 0, "tilted ring loft has no degenerate triangles");
    assert!(report.near_duplicate_pairs == 0 && report.coincident_pairs == 0, "tilted ring loft: no near-duplicate or coincident vertex pairs: {report:?}");
    assert_eq!(pieces(&mesh), 1, "tilted ring loft is one piece");
    assert!(mesh.corner_normals.is_empty(), "ring loft mesh has no corner normals");
}

#[test]
fn ring_loft_layers_have_prismatoid_volume_and_flags_preserve_the_mesh() {
    let rings = vec![square_loft_ring(0.0, 3.0), square_loft_ring(2.0, 2.0), square_loft_ring(5.0, 1.0)];
    let expected: f64 = [(2.0, 6.0, 4.0), (3.0, 4.0, 2.0)].into_iter().map(|(h, a, b)| {
        let mid = (a + b) * 0.5;
        h / 6.0 * (a * a + 4.0 * mid * mid + b * b)
    }).sum();
    let mut g = Graph::new();
    let mut e = Evaluator::new();
    for along_x in [false, true] {
        let mut rings = rings.clone();
        if along_x {
            for ring in &mut rings {
                ring.origin = [ring.origin[2], 1.0, -2.0];
                ring.u = [0.0, 1.0, 0.0];
                ring.v = [0.0, 0.0, 1.0];
            }
        }
        let id = g.push(Op::RingLoft { rings: rings.clone(), prism: false }).unwrap();
        let mesh = e.mesh(&g, id, None).expect("a square ring loft evaluates in its own frame");
        let volume = makepad_csg_cad::signed_volume(&mesh);
        assert!((volume - expected).abs() < expected * 1e-9, "ring loft layer volume (along x: {along_x}): {volume} expected {expected}");
        rings[1].crease = true;
        let creased = g.push(Op::RingLoft { rings, prism: false }).unwrap();
        let creased = e.mesh(&g, creased, None).expect("a creased ring loft evaluates");
        assert_eq!(mesh.vertices, creased.vertices, "crease preserves ring loft vertices");
        assert_eq!(mesh.triangles, creased.triangles, "crease preserves ring loft triangles");
        assert!(creased.corner_normals.is_empty(), "crease adds no ring loft corner normals");
    }
    let rings = vec![square_loft_ring(0.0, 1.0), square_loft_ring(2.0, 1.0)];
    let a = g.push(Op::RingLoft { rings: rings.clone(), prism: false }).unwrap();
    let b = g.push(Op::RingLoft { rings, prism: true }).unwrap();
    let a = e.mesh(&g, a, None).expect("a straight ring loft evaluates");
    let b = e.mesh(&g, b, None).expect("a prism ring loft evaluates");
    assert_eq!(a.vertices, b.vertices, "prism preserves ring loft vertices");
    assert_eq!(a.triangles, b.triangles, "prism preserves ring loft triangles");
    assert!(b.corner_normals.is_empty(), "prism adds no ring loft corner normals");
}

#[test]
fn ring_loft_validity_errors_name_the_failed_rule() {
    let rings = vec![square_loft_ring(0.0, 1.0), square_loft_ring(2.0, 1.0)];
    let mut invalid = vec![(Vec::new(), "at least 2 rings"), (rings[..1].to_vec(), "at least 2 rings")];
    for k in 0..rings.len() {
        for (rule, field) in [("at least 3 points", 0), ("different point counts", 1), ("unit-length u", 2), ("unit-length v", 3), ("orthogonal", 4), ("finite origin", 5), ("finite origin and points", 6)] {
            let mut changed = rings.clone();
            match field {
                0 => changed[k].points.truncate(2),
                1 => changed[k].points.truncate(3),
                2 => changed[k].u = [2.0, 0.0, 0.0],
                3 => changed[k].v = [0.0, 2.0, 0.0],
                4 => changed[k].v = [std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 0.0],
                5 => changed[k].origin[0] = f64::INFINITY,
                _ => changed[k].points[0][0] = f64::NAN,
            }
            invalid.push((changed, rule));
        }
    }
    for z in [0.0, -1.0] {
        let mut changed = rings.clone();
        changed[1].origin[2] = z;
        invalid.push((changed, "strictly increasing along the first ring normal"));
    }
    let mut changed = rings.clone();
    for ring in &mut changed {
        ring.u = [0.0, 1.0, 0.0];
        ring.v = [0.0, 0.0, 1.0];
    }
    invalid.push((changed, "strictly increasing along the first ring normal"));
    for (rings, rule) in invalid {
        let mut g = Graph::new();
        let id = g.push(Op::RingLoft { rings, prism: false }).unwrap();
        let failure = Evaluator::new().mesh(&g, id, None).expect_err(rule);
        assert_eq!(failure.op, "ring_loft", "invalid ring loft errors identify the operation");
        assert!(failure.message.contains(rule), "ring loft error names {rule}: {}", failure.message);
    }
}

#[test]
fn ring_loft_keys_include_every_ring_coordinate_count_and_flag() {
    let rings = vec![square_loft_ring(0.0, 1.0), square_loft_ring(2.0, 1.0)];
    let op = Op::RingLoft { rings: rings.clone(), prism: false };
    assert_eq!(op.name(), "ring_loft", "ring loft has its own operation name");
    assert!(op.inputs().is_empty(), "ring loft has no graph inputs");
    let mut g = Graph::new();
    let mut key_of = |rings, prism| {
        let id = g.push(Op::RingLoft { rings, prism }).unwrap();
        g.node(id).key
    };
    let key = key_of(rings.clone(), false);
    assert_eq!(key, key_of(rings.clone(), false), "identical ring loft operations have identical keys");
    assert_ne!(key, key_of(rings.clone(), true), "prism changes the ring loft key");
    assert_ne!(key, key_of(rings[..1].to_vec(), false), "ring count changes the ring loft key");
    for k in 0..rings.len() {
        for field in ["origin", "u", "v"] {
            for axis in 0..3 {
                let mut changed = rings.clone();
                let value = match field {
                    "origin" => &mut changed[k].origin,
                    "u" => &mut changed[k].u,
                    _ => &mut changed[k].v,
                };
                value[axis] += 0.25;
                assert_ne!(key, key_of(changed, false), "ring {k} {field} axis {axis} changes the ring loft key");
            }
        }
        for i in 0..rings[k].points.len() {
            for axis in 0..2 {
                let mut changed = rings.clone();
                changed[k].points[i][axis] += 0.25;
                assert_ne!(key, key_of(changed, false), "ring {k} point {i} axis {axis} changes the ring loft key");
            }
        }
        let mut changed = rings.clone();
        changed[k].points.pop();
        assert_ne!(key, key_of(changed, false), "ring {k} point count changes the ring loft key");
        let mut changed = rings.clone();
        changed[k].crease = true;
        assert_ne!(key, key_of(changed, false), "ring {k} crease changes the ring loft key");
    }
}

#[test]
fn equal_subgraphs_have_equal_keys_and_build_once() {
    let mut g = Graph::new();
    let a = g.push(Op::Box { size: [10.0, 4.0, 6.0], radius: 0.0 }).unwrap();
    let b = g.push(Op::Box { size: [10.0, 4.0, 6.0], radius: 0.0 }).unwrap();
    let c = g.push(Op::Box { size: [10.0, 4.0, 6.5], radius: 0.0 }).unwrap();
    assert_eq!(g.node(a).key, g.node(b).key);
    assert_ne!(g.node(a).key, g.node(c).key);
    let mut e = Evaluator::new();
    e.begin();
    let ma = e.mesh(&g, a, None).unwrap();
    let mb = e.mesh(&g, b, None).unwrap();
    assert!(Arc::ptr_eq(&ma, &mb));
    assert_eq!(e.built, 1);
}

#[test]
fn operands_must_come_first() {
    let mut g = Graph::new();
    assert!(g.push(Op::Transform { source: NodeId(0), at: [0.0; 3], rotate: [0.0; 3], scale: [1.0; 3] }).is_err());
}

#[test]
fn rounded_box_is_closed_and_inside_its_size() {
    let mut g = Graph::new();
    let a = g.push(Op::Box { size: [20.0, 8.0, 12.0], radius: 1.5 }).unwrap();
    let m = Evaluator::new().mesh(&g, a, None).unwrap();
    assert_eq!(open_edges(&m), 0);
    let bb = m.bounding_box();
    assert!((bb.max.x - 10.0).abs() < 1e-6 && (bb.min.y + 4.0).abs() < 1e-6, "{:?}", bb);
}

#[test]
fn n_ary_difference_cuts_every_tool_and_reuses_the_rest() {
    let mut g = Graph::new();
    let plate = g.push(Op::Extrude { sketch: rect(60.0, 30.0), plane: Plane::Xz, height: 4.0, draft: 0.0, both: false }).unwrap();
    let mut holes = Vec::new();
    for x in [-20.0, 0.0, 20.0] {
        let c = g.push(Op::Cylinder { radius: 2.0, top_radius: 2.0, height: 10.0, segments: 32 }).unwrap();
        holes.push(g.push(Op::Transform { source: c, at: [x, 2.0, 0.0], rotate: [0.0; 3], scale: [1.0; 3] }).unwrap());
    }
    let cut = g.push(Op::Boolean { mode: BoolMode::Difference, a: plate, b: holes }).unwrap();
    let mut e = Evaluator::new();
    e.begin();
    let m = e.mesh(&g, cut, None).unwrap();
    assert_eq!(open_edges(&m), 0);
    // The cylinder is built once; its three placements and the plate and the cut are the rest.
    assert_eq!(e.built, 6);
    let full = 60.0 * 30.0 * 4.0;
    let v = makepad_csg_cad::signed_volume(&m);
    assert!(v < full - 3.0 * 3.1 * 4.0 * 3.5 && v > full - 3.0 * 3.2 * 4.0 * 4.5, "{v}");
    // A changed hole rebuilds that hole's placement and the cut only.
    let mut g2 = Graph::new();
    let plate2 = g2.push(g.node(plate).op.clone()).unwrap();
    let mut holes2 = Vec::new();
    for (i, x) in [-20.0, 0.0, 20.0].into_iter().enumerate() {
        let r = if i == 1 { 2.5 } else { 2.0 };
        let c = g2.push(Op::Cylinder { radius: r, top_radius: r, height: 10.0, segments: 32 }).unwrap();
        holes2.push(g2.push(Op::Transform { source: c, at: [x, 2.0, 0.0], rotate: [0.0; 3], scale: [1.0; 3] }).unwrap());
    }
    let cut2 = g2.push(Op::Boolean { mode: BoolMode::Difference, a: plate2, b: holes2 }).unwrap();
    e.begin();
    e.mesh(&g2, cut2, None).unwrap();
    assert_eq!(e.built, 3);
}

#[test]
fn transforms_rotate_x_then_y_then_z() {
    let r = eval::rotation_xyz([90.0, 0.0, 0.0]);
    // +Y turns to +Z about X.
    let y = [r[0][1], r[1][1], r[2][1]];
    assert!((y[2] - 1.0).abs() < 1e-12 && y[1].abs() < 1e-12);
}

#[test]
fn a_field_meshes_inside_its_bounds() {
    use makepad_script::*;
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let field = vm.eval(ScriptMod::new("", "", "ball_field", 0, 0,
        "use mod.math.*\nlet field = fn(p) { return length(p) - 10.0 }\n(field)", &[])).as_object().unwrap();
    let _keep = vm.new_object_ref(field);
    let field = makepad_csg_sdf::SdfSplashExpr::compile(&mut vm, field, makepad_csg_sdf::SplashPoint::Vec3, &[]).unwrap();
    let mut g = Graph::new();
    let shape = SdfShape { root: SdfNode::Kernel(SdfKernel::splash(field, Some(([-10.0; 3], [10.0; 3])))), min: [-11.0; 3], max: [11.0; 3], materials: Vec::new() };
    let f = g.push(Op::Field { shape, cell: Some(1.0) }).unwrap();
    let mut e = Evaluator::new();
    let m = e.mesh(&g, f, None).unwrap();
    assert_eq!(open_edges(&m), 0);
    let v = makepad_csg_cad::signed_volume(&m);
    let ball = 4.0 / 3.0 * std::f64::consts::PI * 1000.0;
    assert!((v - ball).abs() < ball * 0.03, "{v}");
    // The draft is part of the cache key.
    e.draft = 2.0;
    e.begin();
    let coarse = e.mesh(&g, f, None).unwrap();
    assert!(coarse.triangles.len() < m.triangles.len());
}

/// A cage takes one level fewer at a live-editing draft, at its steps or at its chord, and
/// the draft is part of its cache key.
#[test]
fn a_cage_drafts_one_level_fewer() {
    let points = vec![[-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0]];
    let faces = vec![vec![0, 3, 2, 1], vec![4, 5, 6, 7], vec![0, 1, 5, 4], vec![2, 3, 7, 6], vec![1, 2, 6, 5], vec![0, 4, 7, 3]];
    // Each level quadruples the quads: six faces, two triangles each.
    let tris = |levels: u32| 12 * 4usize.pow(levels);
    let mut g = Graph::new();
    let steps = g.push(Op::Cage { points: points.clone(), faces: faces.clone(), levels: Some(3) }).unwrap();
    let chord = g.push(Op::Cage { points, faces, levels: None }).unwrap();
    // At a 0.03 chord the cube takes three levels, its points 0.012 from the limit after them
    // and 0.048 after two.
    let mut e = Evaluator::new();
    e.brep_chord = 0.03;
    assert_eq!(e.mesh(&g, chord, None).unwrap().triangles.len(), tris(3));
    assert_eq!(e.mesh(&g, steps, None).unwrap().triangles.len(), tris(3));
    e.draft = 2.0;
    e.begin();
    assert_eq!(e.mesh(&g, steps, None).unwrap().triangles.len(), tris(2));
    assert_eq!(e.mesh(&g, chord, None).unwrap().triangles.len(), tris(2));
}

#[test]
fn analytic_shapes_mesh_without_a_vm() {
    let shape = SdfShape {
        root: SdfNode::Combine {
            op: SdfOp::Union,
            blend: 4.0,
            of: vec![SdfNode::Sphere { radius: 10.0 }, SdfNode::Transform { at: [12.0, 0.0, 0.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Box { size: [30.0, 6.0, 6.0], radius: 0.0 }) }],
        },
        min: [-11.0, -11.0, -11.0],
        max: [28.0, 11.0, 11.0],
        materials: Vec::new(),
    };
    let mut g = Graph::new();
    let f = g.push(Op::Field { shape, cell: Some(1.0) }).unwrap();
    let m = Evaluator::new().mesh(&g, f, None).unwrap();
    assert_eq!(open_edges(&m), 0);
    assert!(makepad_csg_cad::signed_volume(&m) > 4.0 / 3.0 * std::f64::consts::PI * 1000.0);
}

#[test]
fn meshes_built_ahead_are_the_meshes_mesh_builds() {
    let ball = |radius: f64| Op::Field { shape: SdfShape { root: SdfNode::Sphere { radius }, min: [-radius - 1.0; 3], max: [radius + 1.0; 3], materials: Vec::new() }, cell: Some(1.0) };
    let at = |g: &mut Graph, source: NodeId, x: f64, y: f64| g.push(Op::Transform { source, at: [x, y, 0.0], rotate: [0.0; 3], scale: [1.0; 3] }).unwrap();
    let mut g = Graph::new();
    // two wheel-like parts: a field and a mesh made one
    let ball_a = g.push(ball(10.0)).unwrap();
    let axle_a = g.push(Op::Cylinder { radius: 4.0, top_radius: 4.0, height: 30.0, segments: 24 }).unwrap();
    let wheel_a = g.push(Op::Boolean { mode: BoolMode::Union, a: ball_a, b: vec![axle_a] }).unwrap();
    let ball_b = g.push(ball(8.0)).unwrap();
    let axle_b = g.push(Op::Cylinder { radius: 3.0, top_radius: 3.0, height: 25.0, segments: 24 }).unwrap();
    let union_b = g.push(Op::Boolean { mode: BoolMode::Union, a: ball_b, b: vec![axle_b] }).unwrap();
    let wheel_b = at(&mut g, union_b, 40.0, 0.0);
    // a part that builds the first wheel's field too (one group with it)
    let spare = at(&mut g, ball_a, 0.0, 40.0);
    // a part over a mesh already kept
    let block = g.push(Op::Box { size: [20.0, 20.0, 20.0], radius: 0.0 }).unwrap();
    let drilled = g.push(Op::Boolean { mode: BoolMode::Difference, a: block, b: vec![axle_b] }).unwrap();
    // a field with nothing inside its box: its mesh fails
    let empty = g.push(Op::Field { shape: SdfShape { root: SdfNode::Sphere { radius: 1.0 }, min: [20.0; 3], max: [22.0; 3], materials: Vec::new() }, cell: Some(1.0) }).unwrap();
    let ids = [wheel_a, wheel_b, spare, drilled, empty];
    let bits = |m: &TriMesh| {
        let b = |v: &makepad_csg_math::Vec3d| [v.x, v.y, v.z].map(f64::to_bits);
        (m.vertices.iter().map(b).collect::<Vec<_>>(), m.triangles.clone(), m.corner_normals.iter().map(b).collect::<Vec<_>>())
    };
    let mut one = Evaluator::new();
    one.begin();
    one.mesh(&g, block, None).unwrap();
    let serial: Vec<_> = ids.iter().map(|id| one.mesh(&g, *id, None).map(|m| bits(&m))).collect();
    assert!(serial[4].is_err());
    let mut ahead = Evaluator::new();
    ahead.begin();
    ahead.mesh(&g, block, None).unwrap();
    ahead.mesh_ahead(&g, &ids, None);
    if makepad_csg_math::thread_pool::thread_count() >= 2 {
        // all but the failing field built ahead, each once
        assert_eq!(ahead.built, one.built);
    }
    let after: Vec<_> = ids.iter().map(|id| ahead.mesh(&g, *id, None).map(|m| bits(&m))).collect();
    assert_eq!(after, serial);
    assert_eq!(ahead.built, one.built);
}

/// An offset's mesh is its source's grown by the distance on every side.
#[test]
fn an_offset_grows_a_mesh_by_its_distance() {
    let mut g = Graph::new();
    let a = g.push(Op::Box { size: [10.0, 4.0, 6.0], radius: 0.0 }).unwrap();
    let grown = g.push(Op::Offset { source: a, distance: 1.0, cell: Some(0.25) }).unwrap();
    let shrunk = g.push(Op::Offset { source: a, distance: -1.0, cell: Some(0.25) }).unwrap();
    let extent = |id| {
        let mesh = Evaluator::new().mesh(&g, id, None).unwrap();
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for v in &mesh.vertices {
            for (k, x) in [v.x, v.y, v.z].into_iter().enumerate() {
                lo[k] = lo[k].min(x);
                hi[k] = hi[k].max(x);
            }
        }
        std::array::from_fn::<f64, 3, _>(|k| hi[k] - lo[k])
    };
    let (up, down) = (extent(grown), extent(shrunk));
    for (k, side) in [10.0, 4.0, 6.0].into_iter().enumerate() {
        assert!((up[k] - (side + 2.0)).abs() < 0.3, "axis {k}: {} grown", up[k]);
        assert!((down[k] - (side - 2.0)).abs() < 0.3, "axis {k}: {} shrunk", down[k]);
    }
}

fn tag_geometry(case: &str, tagged: &TriMesh, plain: &TriMesh) {
    let bits = |ps: &[makepad_csg_math::Vec3d]| ps.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect::<Vec<_>>();
    assert_eq!(bits(&tagged.vertices), bits(&plain.vertices), "{case}: tags leave vertex bits unchanged");
    assert_eq!(tagged.triangles, plain.triangles, "{case}: tags leave triangle indices unchanged");
    assert_eq!(bits(&tagged.corner_normals), bits(&plain.corner_normals), "{case}: tags leave exact normals unchanged");
}

fn tag_centroids(case: &str, out: &TriMesh, sources: &[&TriMesh], reach: f64) {
    use makepad_csg_mesh::bvh::closest_on_tri;
    assert!(!out.triangles.is_empty(), "{case}: the result has a surface");
    assert_eq!(out.tags.len(), out.triangle_count(), "{case}: each triangle has one tag");
    for ti in 0..out.triangle_count() {
        let (a, b, c) = out.triangle_vertices(ti);
        let p = a + ((b - a) + (c - a)) / 3.0;
        let tag = out.tag(ti);
        let mut best = f64::INFINITY;
        for source in sources {
            for si in 0..source.triangle_count() {
                if source.tag(si) == tag {
                    let (a, b, c) = source.triangle_vertices(si);
                    best = best.min(p.distance(closest_on_tri(p, a, b, c)));
                }
            }
        }
        assert!(best <= reach, "{case}: triangle {ti}, tag {tag}, same-tag source distance {best} exceeds {reach}");
    }
}

#[test]
fn tags_follow_rebuilt_surfaces_and_leave_plain_graphs_untagged() {
    use makepad_csg_math::dvec3;
    use makepad_csg_mesh::bvh::Bvh;
    makepad_csg_math::thread_pool::with_serial(|| {
        // How far a result's surface lies from its source. A field-meshed one (shell, offset)
        // lies in cells the field's surface crosses (each fan point is kept inside its cell, its
        // other corners are on the cell's edges), so it may be off by up to a cell's diagonal.
        let cell_diagonal = |cell: f64| 3f64.sqrt() * cell;
        let ops = |source| [
            (Op::Fillet { source, radius: 0.2, edges: edges::EdgeSel::all() }, 0.2),
            (Op::Chamfer { source, size: 0.2, edges: edges::EdgeSel::all() }, 0.2),
            (Op::Shell { source, thickness: 0.25, open: Open::Closed, cell: Some(0.2) }, 0.25 + cell_diagonal(0.2)),
            (Op::Offset { source, distance: 0.2, cell: Some(0.3) }, 0.2 + cell_diagonal(0.3)),
            (Op::Offset { source, distance: -0.25, cell: Some(0.2) }, 0.25 + cell_diagonal(0.2)),
        ];
        let mut box_mesh = makepad_csg_primitives::cube(dvec3(2.0, 2.0, 2.0), true);
        box_mesh.tags = (0..box_mesh.triangle_count()).map(|ti| 1 + ti as u32 / 2).collect();
        let mut g = Graph::new();
        let faces = g.push(Op::Mesh { mesh: Arc::new(box_mesh) }).unwrap();
        let uniform = g.push(Op::Tag { source: faces, tag: u32::MAX }).unwrap();
        let mut plain_g = Graph::new();
        let plain_box = plain_g.push(Op::Box { size: [2.0; 3], radius: 0.0 }).unwrap();
        let plain_source = plain_g.push(Op::Tag { source: plain_box, tag: 0 }).unwrap();
        let mut e = Evaluator::new();
        e.tolerance.min_segments = 8;
        let mut plain_e = Evaluator::new();
        plain_e.tolerance = e.tolerance;
        for (index, (plain_op, reach)) in ops(plain_source).into_iter().enumerate() {
            let name = plain_op.name();
            let plain_id = plain_g.push(plain_op).unwrap();
            let plain = plain_e.mesh(&plain_g, plain_id, None).unwrap_or_else(|err| panic!("plain {name}: evaluation failed: {err}"));
            for (label, source) in [("face tags", faces), ("uniform Tag", uniform)] {
                let (op, _) = ops(source).into_iter().nth(index).unwrap();
                let id = g.push(op).unwrap();
                let out = e.mesh(&g, id, None).unwrap_or_else(|err| panic!("{label} {name}: evaluation failed: {err}"));
                let source = e.mesh(&g, source, None).unwrap();
                let case = format!("{label} {name}");
                tag_geometry(&case, &out, &plain);
                tag_centroids(&case, &out, &[&source], reach + e.tolerance.chord);
                let bvh = Bvh::build(&out);
                for ti in (0..source.triangle_count()).step_by(2) {
                    let center = source.triangle_normal(ti);
                    let (distance, _, nearest) = bvh.closest(center).unwrap();
                    assert!(distance <= reach + e.tolerance.chord, "{case}: face centre has a nearby output surface");
                    assert_eq!(out.tag(nearest as usize), source.tag(ti), "{case}: face centre keeps its exact face tag");
                }
            }
        }
        for i in 0..plain_g.len() {
            let node = NodeId(i as u32);
            let out = plain_e.mesh(&plain_g, node, None).unwrap();
            assert_eq!(out.tags.capacity(), 0, "plain {}: no tag storage is allocated", plain_g.node(node).op.name());
        }
    });
}

#[test]
fn field_slots_tag_overrides_and_keys() {
    makepad_csg_math::thread_pool::with_serial(|| {
        let root = |slots: bool| SdfNode::Id { id: 19, of: Box::new(SdfNode::Combine {
            op: SdfOp::Union, blend: 0.2,
            of: [(-0.7, 5), (0.7, 13)].into_iter().map(|(x, slot)| {
                let ball = SdfNode::Sphere { radius: 1.0 };
                SdfNode::Transform { at: [x, 0.0, 0.0], rotate: [0.0; 3], scale: 1.0,
                    of: Box::new(if slots { SdfNode::Material { slot, within: None, of: Box::new(ball) } } else { ball }) }
            }).collect(),
        }) };
        let mut g = Graph::new();
        let mut field = |slots| g.push(Op::Field { shape: SdfShape { root: root(slots), min: [-2.0, -1.25, -1.25], max: [2.0, 1.25, 1.25], materials: Vec::new() }, cell: Some(0.25) }).unwrap();
        let slots = field(true);
        let plain = field(false);
        let tagged = g.push(Op::Tag { source: slots, tag: u32::MAX }).unwrap();
        let changed = g.push(Op::Tag { source: slots, tag: 17 }).unwrap();
        let other_source = g.push(Op::Tag { source: plain, tag: 17 }).unwrap();
        let cleared = g.push(Op::Tag { source: tagged, tag: 0 }).unwrap();
        assert_eq!(g.node(tagged).op.name(), "tag", "Tag: operation name");
        assert_eq!(g.node(tagged).op.inputs(), [slots], "Tag: source dependency");
        assert_ne!(g.node(tagged).key, g.node(changed).key, "Tag key: tag value participates");
        assert_ne!(g.node(changed).key, g.node(other_source).key, "Tag key: source key participates");
        assert!(Graph::new().push(Op::Tag { source: NodeId(0), tag: 1 }).is_err(), "Tag dependency: future source is rejected");
        let mut e = Evaluator::new();
        let out = e.mesh(&g, slots, None).unwrap();
        assert_eq!(out.tags.len(), out.triangle_count(), "field slots: complete tag channel");
        let mut poles = [0; 2];
        for ti in 0..out.triangle_count() {
            let (a, b, c) = out.triangle_vertices(ti);
            let x = (a.x + b.x + c.x) / 3.0;
            for (i, is_pole, slot) in [(0, x < -1.35, 5), (1, x > 1.35, 13)] {
                if is_pole {
                    poles[i] += 1;
                    assert_eq!(out.tag(ti), slot, "field pole {i}: triangle keeps its sphere slot");
                }
            }
        }
        assert!(poles.iter().all(|&n| n > 0), "field slots: both poles were sampled");
        let plain = e.mesh(&g, plain, None).unwrap();
        assert_eq!(plain.tags.capacity(), 0, "plain field: no tag storage is allocated");
        tag_geometry("field slots", &out, &plain);
        let override_mesh = e.mesh(&g, tagged, None).unwrap();
        assert!(override_mesh.tags.iter().all(|&tag| tag == u32::MAX), "Tag on field: every slot is overridden with the full u32 value");
        tag_geometry("Tag on field", &override_mesh, &out);
        let cleared = e.mesh(&g, cleared, None).unwrap();
        assert_eq!(cleared.tags.capacity(), 0, "Tag 0: channel is cleared without storage");
        tag_geometry("Tag 0", &cleared, &out);
        let a = g.push(Op::Mesh { mesh: out.clone() }).unwrap();
        let b = g.push(Op::Mesh { mesh: override_mesh.clone() }).unwrap();
        assert_ne!(g.node(a).key, g.node(b).key, "imported mesh key: tags distinguish identical geometry");
        assert_eq!(e.mesh(&g, a, None).unwrap().tags, out.tags, "imported slots: cache preserves the original tags");
        assert_eq!(e.mesh(&g, b, None).unwrap().tags, override_mesh.tags, "imported override: cache does not return stale tags");
    });
}

#[test]
fn tagged_booleans_holes_mirrors_and_gear_lookup() {
    use makepad_csg_math::dvec3;
    makepad_csg_math::thread_pool::with_serial(|| {
        let graph = |tagged: bool| {
            let mut g = Graph::new();
            let a = g.push(Op::Box { size: [2.0; 3], radius: 0.0 }).unwrap();
            let a = g.push(Op::Transform { source: a, at: [-0.6, 0.0, 0.0], rotate: [0.0; 3], scale: [1.0; 3] }).unwrap();
            let a = g.push(Op::Tag { source: a, tag: if tagged { 11 } else { 0 } }).unwrap();
            let b = g.push(Op::Box { size: [2.0; 3], radius: 0.0 }).unwrap();
            let b = g.push(Op::Tag { source: b, tag: if tagged { 23 } else { 0 } }).unwrap();
            let b = g.push(Op::Transform { source: b, at: [0.6, 0.2, 0.1], rotate: [0.0; 3], scale: [1.0; 3] }).unwrap();
            let union = g.push(Op::Boolean { mode: BoolMode::Union, a, b: vec![b] }).unwrap();
            g.push(Op::Boolean { mode: BoolMode::Difference, a, b: vec![b] }).unwrap();
            g.push(Op::Boolean { mode: BoolMode::Intersection, a, b: vec![b] }).unwrap();
            let hole = g.push(Op::Hole { source: union, kind: HoleKind::Simple, diameter: 0.5, depth: None, at: vec![[-0.6, 1.0, 0.0]], dir: [0.0, -1.0, 0.0] }).unwrap();
            let reflected = g.push(Op::Mirror { source: hole, plane: MirrorPlane::Yz, center: [1.0, 0.0, 0.0], keep: false }).unwrap();
            let mirror = g.push(Op::Mirror { source: hole, plane: MirrorPlane::Yz, center: [1.0, 0.0, 0.0], keep: true }).unwrap();
            (g, [a, b, union, hole, reflected, mirror])
        };
        let (g, [a, b, union, hole, reflected, mirror]) = graph(true);
        let (plain_g, _) = graph(false);
        let mut e = Evaluator::new();
        e.tolerance.min_segments = 8;
        let mut plain_e = Evaluator::new();
        plain_e.tolerance = e.tolerance;
        for i in 0..g.len() {
            let id = NodeId(i as u32);
            let out = e.mesh(&g, id, None).unwrap();
            let plain = plain_e.mesh(&plain_g, id, None).unwrap();
            let case = g.node(id).op.name();
            tag_geometry(case, &out, &plain);
            assert_eq!(plain.tags.capacity(), 0, "plain {case}: every node has an unallocated tag channel");
        }
        let (a, b) = (e.mesh(&g, a, None).unwrap(), e.mesh(&g, b, None).unwrap());
        let combined = e.mesh(&g, union, None).unwrap();
        tag_centroids("Tag union Tag", &combined, &[&a, &b], 1e-4);
        let bb = combined.bounding_box();
        let depth = (bb.max - bb.min).length() + 2.0;
        let mut tool = makepad_csg_cad::hole_tool(&makepad_csg_cad::Hole::Simple { diameter: 0.5, depth }, 1.0, &e.tolerance).unwrap();
        for p in &mut tool.vertices {
            *p = dvec3(-0.6 + p.y, 1.0 + p.z, p.x);
        }
        let drilled = e.mesh(&g, hole, None).unwrap();
        tag_centroids("Hole after tagged union", &drilled, &[&a, &b, &tool], 1e-4);
        assert!(drilled.tags.contains(&0), "Hole after tagged union: new cutter faces keep tag 0");
        let reflection = e.mesh(&g, reflected, None).unwrap();
        assert_eq!(reflection.tags, drilled.tags, "Mirror without keep: tags keep their triangle order");
        let mirrored = e.mesh(&g, mirror, None).unwrap();
        tag_centroids("Mirror with keep after Hole", &mirrored, &[&drilled, &reflection], 1e-4);
        for (name, out) in [("Union", &combined), ("Hole", &drilled), ("Mirror with keep", &mirrored)] {
            assert!(out.tags.contains(&11) && out.tags.contains(&23), "{name}: both input tags survive");
        }

        let gear = GearSpec { module: 1.0, teeth: 8, thickness: 1.0, pressure: 20.0, backlash: 0.0, shift: 0.0, bore: 0.0 };
        let mut gears = Graph::new();
        let first = gears.push(Op::Gear { gear: gear.clone(), plane: Plane::Xz, mesh_with: None }).unwrap();
        let first = gears.push(Op::Tag { source: first, tag: 7 }).unwrap();
        let second = gears.push(Op::Gear { gear, plane: Plane::Xz, mesh_with: Some((first, 0.0)) }).unwrap();
        let out = e.mesh(&gears, second, None).expect("gear through Tag: source gear lookup succeeds");
        assert!(!out.is_tagged(), "gear through Tag: an adjacent gear is a new untagged source");
    });
}

#[test]
fn tags_cross_the_exact_body_engine_boundary() {
    use std::sync::Mutex;
    // Fixed engine replies isolate recipe keys, chord forwarding and tag projection from
    // the exact kernel. The shared replies must never be modified by the evaluator.
    struct Replies {
        meshes: HashMap<Key, Arc<TriMesh>>,
        calls: Mutex<Vec<(Key, f64)>>,
    }
    impl BrepEngine for Replies {
        fn mesh(&self, steps: Vec<BrepStep>, chord: f64, _: Option<&dyn Fn() -> bool>) -> Result<Arc<TriMesh>, BrepFailure> {
            let mut seen = std::collections::HashSet::new();
            for step in &steps {
                assert!(step.inputs.iter().all(|key| seen.contains(key)), "exact recipe: every input names an earlier body, including through Tag");
                seen.insert(step.key);
            }
            let key = steps.last().expect("exact recipe: a body step is present").key;
            self.calls.lock().unwrap().push((key, chord));
            Ok(self.meshes[&key].clone())
        }
        fn measure(&self, _: Vec<BrepStep>) -> Result<BrepMeasure, BrepFailure> {
            Err(BrepFailure { step: None, lost: None, message: "fixture has only tessellations".into() })
        }
        fn entities(&self, _: Vec<BrepStep>) -> Result<(Vec<BrepEntity>, Vec<BrepEntity>), BrepFailure> {
            Err(BrepFailure { step: None, lost: None, message: "fixture has only tessellations".into() })
        }
        fn at(&self, _: Vec<BrepStep>, _: [f64; 3], _: f64) -> Result<BrepAt, BrepFailure> {
            Err(BrepFailure { step: None, lost: None, message: "fixture has only tessellations".into() })
        }
        fn built(&self, _: &[Key]) -> bool { false }
        fn taken(&self, _: Key) -> Option<Vec<BrepTaken>> { None }
        fn note(&self, _: Key) -> Option<StepNote> { None }
    }
    let mut g = Graph::new();
    let body = g.push(Op::Brep { op: BrepOp::Box { size: [1.0; 3] }, inputs: Vec::new() }).unwrap();
    let tagged = g.push(Op::Tag { source: body, tag: 31 }).unwrap();
    let tagged = g.push(Op::Tag { source: tagged, tag: 47 }).unwrap();
    let cleared = g.push(Op::Tag { source: tagged, tag: 0 }).unwrap();
    let base = Arc::new(makepad_csg_mesh::make_unit_cube());
    let mut replies = Replies { meshes: HashMap::from([(g.node(body).key, base.clone())]), calls: Mutex::new(Vec::new()) };
    let mut ids = Vec::new();
    let mut source = tagged;
    for op in [
        BrepOp::Fillet { radius: 0.1, edges: edges::EdgeSel::all() },
        BrepOp::Chamfer { size: 0.1, edges: edges::EdgeSel::all() },
        BrepOp::Shell { thickness: 0.1, open: Open::Closed },
        BrepOp::Offset { distance: 0.1 },
        BrepOp::Place { at: [2.0, 3.0, 4.0], rotate: [0.0, 90.0, 0.0], scale: [2.0; 3] },
    ] {
        let label = format!("exact {op:?}");
        let mut reply = (*base).clone();
        if let BrepOp::Place { at, rotate, scale } = &op {
            let m = eval::transform_matrix(*at, *rotate, *scale);
            for p in &mut reply.vertices {
                let q = [p.x, p.y, p.z];
                *p = makepad_csg_math::dvec3(
                    m[0][0] * q[0] + m[0][1] * q[1] + m[0][2] * q[2] + m[0][3],
                    m[1][0] * q[0] + m[1][1] * q[1] + m[1][2] * q[2] + m[1][3],
                    m[2][0] * q[0] + m[2][1] * q[1] + m[2][2] * q[2] + m[2][3],
                );
            }
        }
        let id = g.push(Op::Brep { op, inputs: vec![source] }).unwrap();
        replies.meshes.insert(g.node(id).key, Arc::new(reply));
        ids.push((label, id));
        source = id;
    }
    let plain = g.push(Op::Brep { op: BrepOp::Offset { distance: 0.1 }, inputs: vec![body] }).unwrap();
    let zero = g.push(Op::Brep { op: BrepOp::Offset { distance: 0.1 }, inputs: vec![cleared] }).unwrap();
    for id in [plain, zero] {
        replies.meshes.insert(g.node(id).key, base.clone());
    }
    let engine = Arc::new(replies);
    let mut e = Evaluator::with_brep(engine.clone());
    let chord = 0.125;
    for (label, id) in ids {
        let out = e.brep_mesh(&g, id, chord, None).unwrap().unwrap();
        assert_eq!(out.tags, vec![47; out.triangle_count()], "{label}: the outer Tag reaches the new tessellation");
        tag_geometry(&label, &out, &engine.meshes[&g.node(id).key]);
        let calls = engine.calls.lock().unwrap().len();
        let cached = e.brep_mesh(&g, id, chord, None).unwrap().unwrap();
        assert!(Arc::ptr_eq(&cached, &out), "{label}: the cache keeps the tagged mesh");
        assert_eq!(engine.calls.lock().unwrap().len(), calls, "{label}: a cache hit makes no engine request");
    }
    assert!(engine.calls.lock().unwrap().iter().all(|(_, c)| *c == chord), "exact tags: source tessellations use the requested chord");
    for (label, id) in [("untagged body", plain), ("Tag 0 body", zero)] {
        engine.calls.lock().unwrap().clear();
        let out = Evaluator::with_brep(engine.clone()).brep_mesh(&g, id, chord, None).unwrap().unwrap();
        assert_eq!(out.tags.capacity(), 0, "{label}: no tag storage is allocated");
        assert_eq!(engine.calls.lock().unwrap().len(), 1, "{label}: no source tessellation is requested for tags");
    }
    assert!(engine.meshes.values().all(|mesh| !mesh.is_tagged()), "exact tags: shared engine replies are unchanged");
}


/// Unions whose coplanar caps meet an earlier union's near-coincident points: a light pipe's tip,
/// shoulder and bridge, with their own rings, in both union orders, unturned and turned. The
/// earlier union leaves needle triangles and tiny pieces on the shoulder's top; the bridge's top
/// lies in that plane, the needles there must count as coplanar with it and the tiny pieces must
/// stay, or the seam opens. Turned, the bridge's top edges lie in the cap's plane only up to
/// rounding: they must not cross it at an arbitrary point along their length, whichever operand
/// comes first.
#[test]
fn coplanar_caps_over_an_earlier_unions_needles_stay_closed() {
    use makepad_csg_mesh::validate::validate_mesh;
    fn circle(n: usize, r: f32, z: f32) -> Ring {
        let points = (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                [f64::from(r * a.cos()), f64::from(r * a.sin())]
            })
            .collect();
        Ring { origin: [0.0, 0.0, f64::from(z)], u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], points, crease: true }
    }
    let corners: [[f32; 3]; 8] = [
        [-5.3621483, -1.3797872, 8.6], [-5.3621483, 1.3797872, 8.6], [5.3621483, -1.3797872, 8.6], [5.3621483, 1.3797872, 8.6],
        [-5.299259, -1.4007332, 9.8], [-5.299259, 1.4007332, 9.8], [5.299259, -1.4007332, 9.8], [5.299259, 1.4007332, 9.8],
    ];
    // Turns about one, two and all three axes: every cap leaves its axis plane.
    let turns = [[0.0; 3], [23.7, -61.3, 141.9], [10.0, 0.0, 0.0], [0.0, 0.0, 33.0], [45.0, 45.0, 0.0], [-71.2, 12.9, 5.5], [90.0, 0.0, 0.0]];
    let mut failed = Vec::new();
    for rotate in turns {
        for n in [32, 48, 64] {
            let mut g = Graph::new();
            let mut place = |op: Op| {
                let id = g.push(op).unwrap();
                g.push(Op::Transform { source: id, at: [0.0; 3], rotate, scale: [1.0; 3] }).unwrap()
            };
            let tip = place(Op::RingLoft { rings: vec![circle(n, 1.4392738, 9.75), circle(n, 1.43491, 10.0)], prism: false });
            let shoulder = place(Op::RingLoft { rings: vec![circle(n, 2.903347, 8.6), circle(n, 3.3103292, 9.0), circle(n, 3.3242931, 9.8)], prism: false });
            let bridge = place(Op::Hull { points: corners.iter().map(|p| p.map(f64::from)).collect() });
            // The turned 64-segment case also with each union's operands swapped.
            let swaps: &[bool] = if n == 64 && rotate == turns[1] { &[false, true] } else { &[false] };
            for (order, first, second, third) in [("(tip + shoulder) + bridge", tip, shoulder, bridge), ("(tip + bridge) + shoulder", tip, bridge, shoulder)] {
                for &swap in swaps {
                    let union = |g: &mut Graph, a, b| {
                        let (a, b) = if swap { (b, a) } else { (a, b) };
                        g.push(Op::Boolean { mode: BoolMode::Union, a, b: vec![b] }).unwrap()
                    };
                    let pair = union(&mut g, first, second);
                    let all = union(&mut g, pair, third);
                    let mesh = Evaluator::new().mesh(&g, all, None).expect("the union evaluates");
                    let report = validate_mesh(&mesh);
                    if !(report.is_closed && report.is_manifold && report.degenerate_triangles == 0
                        && report.near_duplicate_pairs == 0 && report.coincident_pairs == 0) {
                        failed.push(format!(
                            "{n} segments, turned {rotate:?}, {order}, operands swapped {swap}: \
                             open {}, non-manifold {}, flat {}, near duplicate pairs {}, coincident pairs {}",
                            report.boundary_edges, report.non_manifold_edges, report.degenerate_triangles,
                            report.near_duplicate_pairs, report.coincident_pairs));
                    }
                }
            }
        }
    }
    assert!(failed.is_empty(), "every union closed, manifold and without flat triangles:\n{}", failed.join("\n"));
}
