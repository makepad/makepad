//! The exact mesh booleans at the finishing tolerance Fab's evaluator uses
//! (one part per million of the parts' diagonal, 0.01 to 1 micron, see
//! [`finish`]). Touching and coplanar faces are resolved by the
//! corefinement's classification.

use makepad_csg_boolean::boolean::{mesh_boolean_in, mesh_boolean_with, BoolOp, FinishParams};
use makepad_csg_boolean::MeshAccel;
use std::sync::OnceLock;
use makepad_csg_mesh::mesh::TriMesh;

/// Weld tolerance for a boolean of `a` and `b`: 1e-6 of their
/// joint diagonal, clamped to 1e-5 .. 1e-3 mm.
fn finish(a: &TriMesh, b: &TriMesh) -> FinishParams {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in a.vertices.iter().chain(b.vertices.iter()) {
        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt().max(1.0);
    let tol = (diag * 1e-6).clamp(1e-5, 1e-3);
    FinishParams { tolerance: tol }
}

fn disjoint(a: &TriMesh, b: &TriMesh) -> bool {
    let (ba, bb) = (a.bounding_box(), b.bounding_box());
    ba.max.x < bb.min.x || bb.max.x < ba.min.x || ba.max.y < bb.min.y || bb.max.y < ba.min.y || ba.max.z < bb.min.z || bb.max.z < ba.min.z
}

pub fn union(a: &TriMesh, b: &TriMesh) -> TriMesh {
    if a.triangles.is_empty() {
        return b.clone();
    }
    if b.triangles.is_empty() {
        return a.clone();
    }
    if disjoint(a, b) {
        let mut out = a.clone();
        out.merge(b);
        return out;
    }
    mesh_boolean_with(a, b, BoolOp::Union, finish(a, b))
}

pub fn difference(a: &TriMesh, b: &TriMesh) -> TriMesh {
    if b.triangles.is_empty() || disjoint(a, b) {
        return a.clone();
    }
    mesh_boolean_with(a, b, BoolOp::Difference, finish(a, b))
}

pub fn intersection(a: &TriMesh, b: &TriMesh) -> TriMesh {
    if disjoint(a, b) {
        return TriMesh::new();
    }
    mesh_boolean_with(a, b, BoolOp::Intersection, finish(a, b))
}

/// One boolean of prepared operands: the same result as [`union`],
/// [`difference`] and [`intersection`], with each operand's triangles and
/// tree built into its `OnceLock` the first time a boolean needs them and
/// reused by every later boolean on the same mesh (an evaluator keeps them
/// with its memoised operands).
pub fn boolean_in(a: &TriMesh, prep_a: &OnceLock<MeshAccel>, b: &TriMesh, prep_b: &OnceLock<MeshAccel>, op: BoolOp) -> TriMesh {
    match op {
        BoolOp::Union if a.triangles.is_empty() => return b.clone(),
        BoolOp::Union if b.triangles.is_empty() => return a.clone(),
        BoolOp::Union if disjoint(a, b) => {
            let mut out = a.clone();
            out.merge(b);
            return out;
        }
        BoolOp::Difference if b.triangles.is_empty() || disjoint(a, b) => return a.clone(),
        BoolOp::Intersection if disjoint(a, b) => return TriMesh::new(),
        _ => {}
    }
    let pa = prep_a.get_or_init(|| MeshAccel::build(a));
    let pb = prep_b.get_or_init(|| MeshAccel::build(b));
    mesh_boolean_in(a, pa, b, pb, op, finish(a, b))
}
