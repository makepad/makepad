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
    struct Ball;
    impl makepad_csg_sdf::Sdf3 for Ball {
        fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
            p.length() - 10.0
        }
    }
    let mut g = Graph::new();
    let shape = SdfShape { root: SdfNode::Kernel(SdfKernel { key: 1, field: Arc::new(Ball) }), min: [-11.0; 3], max: [11.0; 3], materials: Vec::new() };
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
