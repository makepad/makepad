// Triangle-triangle intersection test and segment computation.
//
// Based on Moller's "A Fast Triangle-Triangle Intersection Test" (1997),
// with Shewchuk's orient3d for robust orientation tests.
//
// Algorithm:
// 1. Compute the plane of triangle B.
// 2. Classify all vertices of A against B's plane.
// 3. If all on the same side, no intersection.
// 4. Compute the plane of triangle A.
// 5. Classify all vertices of B against A's plane.
// 6. If all on the same side, no intersection.
// 7. Compute the line of intersection of the two planes.
// 8. Project both triangles onto this line.
// 9. Check if the projected intervals overlap.
// 10. If they do, compute the intersection segment.

use makepad_csg_math::{orient3d, Vec3d};

/// Identifies which edge of which triangle produced an intersection point.
/// The edge is defined by the two mesh-level vertex indices.
/// `other_tri` is the triangle from the other mesh whose plane was intersected.
#[derive(Clone, Debug, Copy, Hash, Eq, PartialEq)]
pub struct EdgeIsect {
    /// Smaller vertex index of the edge
    pub edge_v0: u32,
    /// Larger vertex index of the edge
    pub edge_v1: u32,
    /// Triangle index in the other mesh whose plane defines the intersection
    pub other_tri: u32,
}

/// Result of a triangle-triangle intersection test.
#[derive(Clone, Debug)]
pub struct TriTriResult {
    /// Whether the triangles intersect.
    pub intersects: bool,
    /// Whether the triangles are coplanar.
    pub coplanar: bool,
    /// The intersection segment (two endpoints), if they intersect and aren't coplanar.
    pub segment: Option<(Vec3d, Vec3d)>,
    /// Edge identifiers for each segment endpoint (for edge-centric deduplication).
    /// First element corresponds to segment.0, second to segment.1.
    pub edge_ids: Option<(EdgeIsect, EdgeIsect)>,
}

impl TriTriResult {
    fn no_intersection() -> TriTriResult {
        TriTriResult {
            intersects: false,
            coplanar: false,
            segment: None,
            edge_ids: None,
        }
    }

    fn coplanar_intersection() -> TriTriResult {
        TriTriResult {
            intersects: true,
            coplanar: true,
            segment: None,
            edge_ids: None,
        }
    }
}

/// Test if two triangles intersect and compute the intersection segment.
///
/// Uses robust orient3d predicates for classification.
/// Returns the intersection segment if the triangles properly intersect (not coplanar).
pub fn tri_tri_intersection(
    a0: Vec3d,
    a1: Vec3d,
    a2: Vec3d,
    b0: Vec3d,
    b1: Vec3d,
    b2: Vec3d,
) -> TriTriResult {
    tri_tri_intersection_indexed(a0, a1, a2, [0, 1, 2], 0, b0, b1, b2, [0, 1, 2], 0)
}

/// Test if two triangles intersect, with mesh-level vertex/triangle indices
/// for edge-centric deduplication.
///
/// `a_vi`/`b_vi` are the mesh-level vertex indices of each triangle's vertices.
/// `a_ti`/`b_ti` are the mesh-level triangle indices.
///
/// Returns `edge_ids` that uniquely identify each segment endpoint by the
/// (mesh edge, other triangle) pair that produced it. This allows callers to
/// cache intersection points so the same edge-plane intersection is computed
/// only once.
pub fn tri_tri_intersection_indexed(
    a0: Vec3d,
    a1: Vec3d,
    a2: Vec3d,
    a_vi: [u32; 3],
    a_ti: u32,
    b0: Vec3d,
    b1: Vec3d,
    b2: Vec3d,
    b_vi: [u32; 3],
    b_ti: u32,
) -> TriTriResult {
    // Step 1-2: Classify A vertices against B's plane
    let db0 = orient3d(b0, b1, b2, a0);
    let db1 = orient3d(b0, b1, b2, a1);
    let db2 = orient3d(b0, b1, b2, a2);

    // All on same side? No intersection.
    if db0 > 0.0 && db1 > 0.0 && db2 > 0.0 {
        return TriTriResult::no_intersection();
    }
    if db0 < 0.0 && db1 < 0.0 && db2 < 0.0 {
        return TriTriResult::no_intersection();
    }

    // Step 3-4: Classify B vertices against A's plane
    let da0 = orient3d(a0, a1, a2, b0);
    let da1 = orient3d(a0, a1, a2, b1);
    let da2 = orient3d(a0, a1, a2, b2);

    // All on same side? No intersection.
    if da0 > 0.0 && da1 > 0.0 && da2 > 0.0 {
        return TriTriResult::no_intersection();
    }
    if da0 < 0.0 && da1 < 0.0 && da2 < 0.0 {
        return TriTriResult::no_intersection();
    }

    // Check for coplanar case
    if db0 == 0.0 && db1 == 0.0 && db2 == 0.0 {
        // Coplanar triangles: use 2D overlap test
        if coplanar_tri_tri(a0, a1, a2, b0, b1, b2) {
            return TriTriResult::coplanar_intersection();
        } else {
            return TriTriResult::no_intersection();
        }
    }

    // Step 5: Compute line of intersection of the two planes.
    // The direction is the cross product of the two normals.
    let na = (a1 - a0).cross(a2 - a0);
    let nb = (b1 - b0).cross(b2 - b0);
    let dir = na.cross(nb);

    // Choose the axis with largest projection for numerical stability
    let ax = dir.x.abs();
    let ay = dir.y.abs();
    let az = dir.z.abs();
    let project = if ax >= ay && ax >= az {
        0
    } else if ay >= az {
        1
    } else {
        2
    };

    // Step 6: Project triangles onto the intersection line.
    // Compute interval for triangle A (intersected by B's plane)
    let (a_min, a_max, a_p0, a_p1, a_edge0, a_edge1) =
        compute_interval_indexed(a0, a1, a2, a_vi, db0, db1, db2, b_ti, dir, project);
    // Compute interval for triangle B (intersected by A's plane)
    let (b_min, b_max, b_p0, b_p1, b_edge0, b_edge1) =
        compute_interval_indexed(b0, b1, b2, b_vi, da0, da1, da2, a_ti, dir, project);

    // Step 7: Check interval overlap
    if a_max <= b_min || b_max <= a_min {
        return TriTriResult::no_intersection();
    }

    // Compute intersection segment: the overlap of the two intervals
    let (seg_start, edge_start) = if a_min > b_min {
        (a_p0, a_edge0)
    } else {
        (b_p0, b_edge0)
    };
    let (seg_end, edge_end) = if a_max < b_max {
        (a_p1, a_edge1)
    } else {
        (b_p1, b_edge1)
    };

    TriTriResult {
        intersects: true,
        coplanar: false,
        segment: Some((seg_start, seg_end)),
        edge_ids: Some((edge_start, edge_end)),
    }
}

/// Compute the interval of a triangle projected onto the intersection line,
/// with edge identity tracking for deduplication.
///
/// Returns (t_min, t_max, point_at_min, point_at_max, edge_id_min, edge_id_max).
///
/// `vi` are the mesh-level vertex indices for this triangle.
/// `other_ti` is the mesh-level triangle index of the other triangle whose plane is used.
fn compute_interval_indexed(
    v0: Vec3d,
    v1: Vec3d,
    v2: Vec3d,
    vi: [u32; 3],
    d0: f64,
    d1: f64,
    d2: f64,
    other_ti: u32,
    dir: Vec3d,
    project: usize,
) -> (f64, f64, Vec3d, Vec3d, EdgeIsect, EdgeIsect) {
    // Find the vertex on one side and the two on the other.
    let (iso, iso_d, va, va_d, vb, vb_d, iso_i, va_i, vb_i) =
        isolate_vertex_indexed(v0, v1, v2, d0, d1, d2, vi);

    // Compute intersection points on edges iso->va and iso->vb
    let pa = edge_plane_intersection(iso, va, iso_d, va_d);
    let pb = edge_plane_intersection(iso, vb, iso_d, vb_d);

    // Build edge identifiers (canonical: smaller index first)
    let edge_a = EdgeIsect {
        edge_v0: iso_i.min(va_i),
        edge_v1: iso_i.max(va_i),
        other_tri: other_ti,
    };
    let edge_b = EdgeIsect {
        edge_v0: iso_i.min(vb_i),
        edge_v1: iso_i.max(vb_i),
        other_tri: other_ti,
    };

    // Project onto the intersection line direction
    let ta = project_onto(pa, dir, project);
    let tb = project_onto(pb, dir, project);

    if ta <= tb {
        (ta, tb, pa, pb, edge_a, edge_b)
    } else {
        (tb, ta, pb, pa, edge_b, edge_a)
    }
}

/// Find the isolated vertex (the one on the opposite side from the other two),
/// also returning mesh-level vertex indices.
fn isolate_vertex_indexed(
    v0: Vec3d,
    v1: Vec3d,
    v2: Vec3d,
    d0: f64,
    d1: f64,
    d2: f64,
    vi: [u32; 3],
) -> (Vec3d, f64, Vec3d, f64, Vec3d, f64, u32, u32, u32) {
    let s0 = sign(d0);
    let s1 = sign(d1);
    let s2 = sign(d2);

    // If d0 is isolated (different sign from d1 and d2)
    if s0 != s1 && s0 != s2 {
        return (v0, d0, v1, d1, v2, d2, vi[0], vi[1], vi[2]);
    }
    // If d1 is isolated
    if s1 != s0 && s1 != s2 {
        return (v1, d1, v0, d0, v2, d2, vi[1], vi[0], vi[2]);
    }
    // d2 is isolated (or degenerate case)
    (v2, d2, v0, d0, v1, d1, vi[2], vi[0], vi[1])
}

fn sign(x: f64) -> i32 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Compute the intersection point of edge (a->b) with a plane,
/// given signed distances da, db of a, b from that plane.
fn edge_plane_intersection(a: Vec3d, b: Vec3d, da: f64, db: f64) -> Vec3d {
    let denom = da - db;
    if denom.abs() < 1e-300 {
        return a.lerp(b, 0.5);
    }
    let t = da / denom;
    a.lerp(b, t)
}

/// Project a point onto the intersection line direction using one coordinate.
fn project_onto(p: Vec3d, _dir: Vec3d, axis: usize) -> f64 {
    match axis {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    }
}

/// Test if two coplanar triangles overlap using 2D projected tests.
fn coplanar_tri_tri(a0: Vec3d, a1: Vec3d, a2: Vec3d, b0: Vec3d, b1: Vec3d, b2: Vec3d) -> bool {
    // Project to 2D by dropping the axis most aligned with the shared normal
    let normal = (a1 - a0).cross(a2 - a0);
    let ax = normal.x.abs();
    let ay = normal.y.abs();
    let az = normal.z.abs();

    // Choose which two axes to keep
    let (i, j) = if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    };

    let proj = |v: Vec3d| -> (f64, f64) {
        let coords = [v.x, v.y, v.z];
        (coords[i], coords[j])
    };

    let (a0p, a1p, a2p) = (proj(a0), proj(a1), proj(a2));
    let (b0p, b1p, b2p) = (proj(b0), proj(b1), proj(b2));

    // Test edge-edge crossings
    let a_edges = [(a0p, a1p), (a1p, a2p), (a2p, a0p)];
    let b_edges = [(b0p, b1p), (b1p, b2p), (b2p, b0p)];

    for &ae in &a_edges {
        for &be in &b_edges {
            if segments_intersect_2d(ae.0, ae.1, be.0, be.1) {
                return true;
            }
        }
    }

    // Test point containment (A vertex in B, or B vertex in A)
    if point_in_tri_2d(a0p, b0p, b1p, b2p) {
        return true;
    }
    if point_in_tri_2d(b0p, a0p, a1p, a2p) {
        return true;
    }

    false
}

fn segments_intersect_2d(a0: (f64, f64), a1: (f64, f64), b0: (f64, f64), b1: (f64, f64)) -> bool {
    let d1 = cross_2d(b0, b1, a0);
    let d2 = cross_2d(b0, b1, a1);
    let d3 = cross_2d(a0, a1, b0);
    let d4 = cross_2d(a0, a1, b1);

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }

    // Collinear overlap cases (simplified)
    if d1 == 0.0 && on_segment_2d(b0, b1, a0) {
        return true;
    }
    if d2 == 0.0 && on_segment_2d(b0, b1, a1) {
        return true;
    }
    if d3 == 0.0 && on_segment_2d(a0, a1, b0) {
        return true;
    }
    if d4 == 0.0 && on_segment_2d(a0, a1, b1) {
        return true;
    }

    false
}

fn cross_2d(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn on_segment_2d(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> bool {
    p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
}

fn point_in_tri_2d(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    let d1 = cross_2d(a, b, p);
    let d2 = cross_2d(b, c, p);
    let d3 = cross_2d(c, a, p);

    let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(has_neg && has_pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    #[test]
    fn test_no_intersection_parallel() {
        // Two parallel triangles, no intersection
        let r = tri_tri_intersection(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
            dvec3(0.0, 0.0, 1.0),
            dvec3(1.0, 0.0, 1.0),
            dvec3(0.0, 1.0, 1.0),
        );
        assert!(!r.intersects);
    }

    #[test]
    fn test_no_intersection_separated() {
        // Two triangles far apart
        let r = tri_tri_intersection(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
            dvec3(10.0, 0.0, 0.0),
            dvec3(11.0, 0.0, 0.0),
            dvec3(10.0, 1.0, 0.0),
        );
        assert!(!r.intersects);
    }

    #[test]
    fn test_crossing_intersection() {
        // Two triangles crossing each other
        let r = tri_tri_intersection(
            dvec3(-1.0, -1.0, 0.0),
            dvec3(1.0, -1.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
            dvec3(0.0, 0.0, -1.0),
            dvec3(0.0, 0.0, 1.0),
            dvec3(0.0, 2.0, 0.0),
        );
        assert!(r.intersects);
        assert!(!r.coplanar);
        assert!(r.segment.is_some());
    }

    #[test]
    fn test_coplanar_overlapping() {
        // Two coplanar triangles that overlap
        let r = tri_tri_intersection(
            dvec3(0.0, 0.0, 0.0),
            dvec3(2.0, 0.0, 0.0),
            dvec3(0.0, 2.0, 0.0),
            dvec3(0.5, 0.5, 0.0),
            dvec3(1.5, 0.5, 0.0),
            dvec3(0.5, 1.5, 0.0),
        );
        assert!(r.intersects);
        assert!(r.coplanar);
    }

    #[test]
    fn test_coplanar_not_overlapping() {
        // Two coplanar triangles that don't overlap
        let r = tri_tri_intersection(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
            dvec3(5.0, 0.0, 0.0),
            dvec3(6.0, 0.0, 0.0),
            dvec3(5.0, 1.0, 0.0),
        );
        assert!(!r.intersects);
    }

    #[test]
    fn test_touching_at_edge() {
        // Triangle A: in XY plane, Triangle B: in XZ plane, sharing an edge along X axis
        let r = tri_tri_intersection(
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.5, 1.0, 0.0),
            dvec3(0.0, 0.0, 0.0),
            dvec3(1.0, 0.0, 0.0),
            dvec3(0.5, 0.0, 1.0),
        );
        // They share an edge, should intersect
        assert!(r.intersects);
    }

    #[test]
    fn test_intersection_segment_location() {
        // Horizontal triangle vs vertical triangle crossing through center
        let r = tri_tri_intersection(
            dvec3(-2.0, 0.0, -2.0),
            dvec3(2.0, 0.0, -2.0),
            dvec3(0.0, 0.0, 2.0),
            dvec3(-1.0, -1.0, 0.0),
            dvec3(1.0, -1.0, 0.0),
            dvec3(0.0, 1.0, 0.0),
        );
        assert!(r.intersects);
        assert!(!r.coplanar);
        if let Some((p0, p1)) = r.segment {
            // The segment should be along the y=0 plane intersection
            // Both points should have y close to 0
            assert!(p0.y.abs() < 0.1, "segment y0: {}", p0.y);
            assert!(p1.y.abs() < 0.1, "segment y1: {}", p1.y);
        }
    }
}
