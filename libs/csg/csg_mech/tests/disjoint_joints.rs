//! A part's joint edits cross its whole mesh once per group of cuts or additions, not once per
//! tool, and the solid stays the one the edits make in declaration order.

use makepad_csg_graph::{BoolMode, Evaluator, Graph, NodeId, Op};
use makepad_csg_mech::{joint::assemble, HingeDecl, JointDecl, JointGeom, Solid};
use makepad_csg_mesh::{mesh::TriMesh, volume::mesh_volume};
use makepad_csg_sdf::{mesh_field::{MeshData, MeshField}, Sdf3};

/// The edits as built, one Boolean per tool in order: how the solid lowered before grouping.
fn sequential(s: &Solid, g: &mut Graph, parts: &[NodeId]) -> NodeId {
    let many = |g: &mut Graph, l: &[Solid]| l.iter().map(|s| sequential(s, g, parts)).collect::<Vec<_>>();
    match s {
        Solid::Part(i) => parts[*i],
        Solid::Prim(q) => q.lower(g, 16).unwrap(),
        Solid::Union(l) | Solid::Inter(l) => {
            let mode = if matches!(s, Solid::Union(_)) { BoolMode::Union } else { BoolMode::Intersection };
            let mut ids = many(g, l);
            if ids.len() == 1 { return ids[0]; }
            let a = ids.remove(0);
            g.push(Op::Boolean { mode, a, b: ids }).unwrap()
        }
        Solid::Diff(a, l) => {
            let a = sequential(a, g, parts);
            let b = many(g, l);
            if b.is_empty() { return a; }
            g.push(Op::Boolean { mode: BoolMode::Difference, a, b }).unwrap()
        }
    }
}

/// How many times a Boolean under `root` takes a mesh that reads `base` as its first operand.
fn passes(g: &Graph, base: NodeId, root: NodeId) -> usize {
    let mut used = vec![false; g.len()];
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !std::mem::replace(&mut used[id.0 as usize], true) {
            stack.extend(g.nodes[id.0 as usize].op.inputs());
        }
    }
    let mut reads = vec![false; g.len()];
    let mut n = 0;
    for (i, node) in g.nodes.iter().enumerate() {
        reads[i] = i == base.0 as usize || node.op.inputs().iter().any(|id| reads[id.0 as usize]);
        if let (true, Op::Boolean { a, b, .. }) = (used[i], &node.op) {
            if reads[a.0 as usize] { n += b.len(); }
        }
    }
    n
}

/// Every vertex and triangle centre of each mesh lies on the other's surface, the volumes agree
/// and both are closed.
fn same_solid(a: &TriMesh, b: &TriMesh) {
    assert!((mesh_volume(a) - mesh_volume(b)).abs() < 1e-8, "volumes {} and {}", mesh_volume(a), mesh_volume(b));
    for (source, target) in [(a, b), (b, a)] {
        let field = MeshField::new(MeshData {
            positions: target.vertices.iter().map(|v| [v.x as f32, v.y as f32, v.z as f32]).collect(),
            triangles: target.triangles.clone(),
        });
        assert!(field.watertight, "the jointed solid is closed");
        for t in &source.triangles {
            let p = t.map(|i| source.vertices[i as usize]);
            for q in [p[0], p[1], p[2], (p[0] + p[1] + p[2]) / 3.0] {
                assert!(field.distance(q).abs() < 1e-4, "the surface moved at {q:?}");
            }
        }
    }
}

fn hinge(x: f64, b: usize) -> (JointGeom, JointDecl) {
    let (at, axis) = ([x, 0.0, 0.0], [0.0, 0.0, 1.0]);
    (
        JointGeom::Hinge { at, axis, s0: -2.0, s1: 2.0, r_pin: 0.5, r_plate: 1.0, t_plate: 0.8, c_r: 0.2, c_z: 0.2, limits: None, brake: None, up: [0.0, 1.0, 0.0], floors: [-1.5, -3.0] },
        JointDecl::Hinge(HingeDecl { a: 0, b, at, axis, limits: None, pin: None, plate: None, clearance: None, clearance_axial: None, brake: None, motor: None }),
    )
}

#[test]
fn disjoint_hinges_cross_each_part_once_per_kind() {
    let (joints, decls): (Vec<_>, Vec<_>) = [-12.0, 0.0, 12.0].into_iter().enumerate().map(|(i, x)| hinge(x, i + 1)).unzip();
    let assembled = assemble(4, &joints, &decls, &[]);
    let mut g = Graph::new();
    let body = g.push(Op::Box { size: [40.0, 3.0, 4.0], radius: 0.0 }).unwrap();
    let leaf = g.push(Op::Box { size: [3.0, 6.0, 4.0], radius: 0.0 }).unwrap();
    let parts = [body, leaf, leaf, leaf];
    // Three hinges on one body: each was a cut and three additions, twelve passes over the body.
    let grouped = assembled[0].lower(&mut g, &parts, 16).unwrap();
    let before = sequential(&assembled[0], &mut g, &parts);
    assert_eq!(passes(&g, body, before), 12);
    assert_eq!(passes(&g, body, grouped), 2, "one difference and one union");
    // A leaf's side: two end cuts, its plate and the pin hole through that plate. The hole
    // meets the plate, so it stays after it: three passes, not four.
    let leaf_grouped = assembled[1].lower(&mut g, &parts, 16).unwrap();
    let leaf_before = sequential(&assembled[1], &mut g, &parts);
    assert_eq!(passes(&g, leaf, leaf_before), 4);
    assert_eq!(passes(&g, leaf, leaf_grouped), 3);
    let mut e = Evaluator::new();
    for (grouped, before) in [(grouped, before), (leaf_grouped, leaf_before)] {
        same_solid(&e.mesh(&g, grouped, None).unwrap(), &e.mesh(&g, before, None).unwrap());
    }
}

#[test]
fn an_edit_that_meets_an_earlier_one_keeps_its_place() {
    // Two hinges at the same place: the second's cut meets the first's knuckles and pin, so it
    // may not move before them.
    let (joints, decls): (Vec<_>, Vec<_>) = [0.0, 0.5].into_iter().enumerate().map(|(i, x)| hinge(x, i + 1)).unzip();
    let assembled = assemble(3, &joints, &decls, &[]);
    let mut g = Graph::new();
    let body = g.push(Op::Box { size: [40.0, 3.0, 4.0], radius: 0.0 }).unwrap();
    let leaf = g.push(Op::Box { size: [3.0, 6.0, 4.0], radius: 0.0 }).unwrap();
    let parts = [body, leaf, leaf];
    let grouped = assembled[0].lower(&mut g, &parts, 16).unwrap();
    let before = sequential(&assembled[0], &mut g, &parts);
    assert_eq!(passes(&g, body, grouped), 4, "cut, additions, cut, additions");
    let mut e = Evaluator::new();
    same_solid(&e.mesh(&g, grouped, None).unwrap(), &e.mesh(&g, before, None).unwrap());
}
