//! The exact mesh booleans at the finishing tolerance Fab's evaluator uses
//! (one part per million of the parts' diagonal, 0.01 to 1 micron, see
//! [`finish`]). Touching and coplanar faces are resolved by the
//! corefinement's classification.

use makepad_csg_boolean::boolean::{mesh_boolean_in, mesh_boolean_with, BoolOp, FinishParams};
use makepad_csg_boolean::MeshAccel;
use makepad_csg_math::BBox3d;
use std::sync::OnceLock;
use makepad_csg_mesh::mesh::TriMesh;

/// Weld tolerance for a boolean of `a` and `b`: 1e-6 of their
/// joint diagonal, clamped to 1e-5 .. 1e-3 mm.
fn finish(a: BBox3d, b: BBox3d) -> FinishParams {
    let diag = a.union(b).size().length().max(1.0);
    let tol = (diag * 1e-6).clamp(1e-5, 1e-3);
    FinishParams { tolerance: tol }
}

fn disjoint(a: BBox3d, b: BBox3d, finish: FinishParams) -> bool {
    if a.is_empty() || b.is_empty() {
        return true;
    }
    // Exact separation can be a one-ulp transform gap between touching caps.
    // Only skip corefinement beyond every distance it can treat as coincident:
    // finishing (weld / T-junction repair), and the broad phase's plane tolerance.
    // The latter can exceed the clamped finish tolerance far from the origin.
    let plane_tol = makepad_csg_boolean::tri_tri::plane_tolerance(
        a.min, a.max, a.min, b.min, b.max, b.min,
    );
    let margin = finish.tolerance.max(plane_tol);
    a.min.x - b.max.x > margin || b.min.x - a.max.x > margin
        || a.min.y - b.max.y > margin || b.min.y - a.max.y > margin
        || a.min.z - b.max.z > margin || b.min.z - a.max.z > margin
}

/// Separation sufficient to reorder tools even on a larger part: use the
/// largest finishing tolerance, as well as the coordinate-scaled plane test.
pub fn disjoint_tool_bounds(a: BBox3d, b: BBox3d) -> bool {
    disjoint(a, b, FinishParams { tolerance: 1e-3 })
}

// Compute each box once: both the early-out and finishing need these bounds.
fn bounds(a: &TriMesh, b: &TriMesh) -> (FinishParams, bool) {
    let (a, b) = (a.bounding_box(), b.bounding_box());
    let finish = finish(a, b);
    (finish, disjoint(a, b, finish))
}

pub fn union(a: &TriMesh, b: &TriMesh) -> TriMesh {
    if a.triangles.is_empty() {
        return b.clone();
    }
    if b.triangles.is_empty() {
        return a.clone();
    }
    let (finish, disjoint) = bounds(a, b);
    if disjoint {
        let mut out = a.clone();
        out.merge(b);
        return out;
    }
    mesh_boolean_with(a, b, BoolOp::Union, finish)
}

pub fn difference(a: &TriMesh, b: &TriMesh) -> TriMesh {
    if b.triangles.is_empty() {
        return a.clone();
    }
    let (finish, disjoint) = bounds(a, b);
    if disjoint {
        return a.clone();
    }
    mesh_boolean_with(a, b, BoolOp::Difference, finish)
}

pub fn intersection(a: &TriMesh, b: &TriMesh) -> TriMesh {
    let (finish, disjoint) = bounds(a, b);
    if disjoint {
        return TriMesh::new();
    }
    mesh_boolean_with(a, b, BoolOp::Intersection, finish)
}

/// One boolean of prepared operands: the same result as [`union`],
/// [`difference`] and [`intersection`], with each operand's triangles and
/// tree built into its `OnceLock` the first time a boolean needs them and
/// reused by every later boolean on the same mesh (an evaluator keeps them
/// with its memoised operands).
pub fn boolean_in(a: &TriMesh, prep_a: &OnceLock<MeshAccel>, b: &TriMesh, prep_b: &OnceLock<MeshAccel>, op: BoolOp) -> TriMesh {
    let (finish, disjoint) = bounds(a, b);
    match op {
        BoolOp::Union if a.triangles.is_empty() => return b.clone(),
        BoolOp::Union if b.triangles.is_empty() => return a.clone(),
        BoolOp::Union if disjoint => {
            let mut out = a.clone();
            out.merge(b);
            return out;
        }
        BoolOp::Difference if b.triangles.is_empty() || disjoint => return a.clone(),
        BoolOp::Intersection if disjoint => return TriMesh::new(),
        _ => {}
    }
    let pa = prep_a.get_or_init(|| MeshAccel::build(a));
    let pb = prep_b.get_or_init(|| MeshAccel::build(b));
    mesh_boolean_in(a, pa, b, pb, op, finish)
}
