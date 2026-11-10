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
