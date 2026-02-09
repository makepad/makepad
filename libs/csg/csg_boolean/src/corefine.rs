// Mesh corefinement: split triangles at intersection curves.
//
// Given two triangle meshes A and B, corefinement modifies both meshes so that
// the intersection curve is represented as edges in both meshes. After corefinement,
// both meshes share edges along their intersection, enabling clean boolean operations.
//
// Algorithm:
// 1. AABB broad phase: find candidate triangle pairs.
// 2. Triangle-triangle intersection: compute intersection segments.
// 3. Collect all intersection points and segments per triangle.
// 4. Re-triangulate affected triangles using CDT.

use crate::aabb_tree::AabbTree;
use crate::cdt::{Point2, CDT};
use crate::tri_tri::{tri_tri_intersection_indexed, EdgeIsect};
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;
use std::collections::HashMap;

/// Information about an intersection point.
#[derive(Clone, Debug)]
pub struct IntersectionPoint {
    pub pos: Vec3d,
    /// Which mesh (0=A, 1=B) this point is associated with.
    pub mesh_id: u32,
    /// Triangle index in the source mesh.
    pub tri_idx: u32,
}

/// An intersection segment between two triangles.
#[derive(Clone, Debug)]
pub struct IntersectionSegment {
    /// Triangle index in mesh A.
    pub tri_a: u32,
    /// Triangle index in mesh B.
    pub tri_b: u32,
    /// Endpoints of the intersection segment.
    pub p0: Vec3d,
    pub p1: Vec3d,
    /// Indices into the shared vertex pool.
    pub v0_idx: u32,
    pub v1_idx: u32,
}

/// Result of corefinement: two refined meshes that share edges along their intersection.
#[derive(Clone, Debug)]
pub struct CorefinementResult {
    /// Refined mesh A.
    pub mesh_a: TriMesh,
    /// Refined mesh B.
    pub mesh_b: TriMesh,
    /// For each triangle in mesh_a, which original triangle it came from.
    pub origin_a: Vec<u32>,
    /// For each triangle in mesh_b, which original triangle it came from.
    pub origin_b: Vec<u32>,
    /// For each triangle in mesh_a, is it on the intersection boundary?
    pub on_boundary_a: Vec<bool>,
    /// For each triangle in mesh_b, is it on the intersection boundary?
    pub on_boundary_b: Vec<bool>,
}

/// Perform mesh corefinement on two triangle meshes.
///
/// Both meshes are split along their intersection curve so that the intersection
/// is represented as edges in both output meshes.
pub fn corefine(mesh_a: &TriMesh, mesh_b: &TriMesh) -> CorefinementResult {
    // Step 1: Build AABB trees
    let tris_a: Vec<_> = (0..mesh_a.triangle_count())
        .map(|i| mesh_a.triangle_vertices(i))
        .collect();
    let tris_b: Vec<_> = (0..mesh_b.triangle_count())
        .map(|i| mesh_b.triangle_vertices(i))
        .collect();

    let tree_a = AabbTree::build(&tris_a);
    let tree_b = AabbTree::build(&tris_b);

    // Step 2: Find candidate pairs
    let pairs = tree_a.find_overlaps(&tree_b);

    // Step 3: Compute intersection segments for each pair.
    //
    // Edge-centric deduplication: when two triangles from mesh A share an edge
    // and both intersect the same triangle from mesh B, the intersection point
    // on that shared edge should be identical. Without deduplication, floating-
    // point differences produce two slightly different points (~1e-5 apart),
    // creating sliver triangles and non-manifold edges. We cache each
    // (edge, other_triangle) intersection so it's computed exactly once.
    let mut segments: Vec<IntersectionSegment> = Vec::new();
    let mut shared_verts: Vec<Vec3d> = Vec::new();
    let mut edge_cache: HashMap<EdgeIsect, Vec3d> = HashMap::new();

    for &(ai, bi) in &pairs {
        let (a0, a1, a2) = tris_a[ai as usize];
        let (b0, b1, b2) = tris_b[bi as usize];
        let a_vi = mesh_a.triangles[ai as usize];
        let b_vi = mesh_b.triangles[bi as usize];

        let result = tri_tri_intersection_indexed(a0, a1, a2, a_vi, ai, b0, b1, b2, b_vi, bi);
        if result.intersects {
            if result.coplanar {
                // Coplanar overlapping triangles: compute the boundary of their
                // 2D intersection polygon (Sutherland-Hodgman clipping) and add
                // each edge as a constraint segment for both triangles.
                let clip_edges = coplanar_intersection_edges(a0, a1, a2, b0, b1, b2);
                for (p0, p1) in clip_edges {
                    if p0.distance(p1) < 1e-12 {
                        continue;
                    }
                    let v0_idx = add_shared_vert(&mut shared_verts, p0, 1e-10);
                    let v1_idx = add_shared_vert(&mut shared_verts, p1, 1e-10);
                    if v0_idx != v1_idx {
                        segments.push(IntersectionSegment {
                            tri_a: ai,
                            tri_b: bi,
                            p0,
                            p1,
                            v0_idx,
                            v1_idx,
                        });
                    }
                }
            } else if let (Some((p0, p1)), Some((eid0, eid1))) = (result.segment, result.edge_ids) {
                // Use cached positions for edge-centric deduplication.
                // If this (edge, other_tri) combo was already computed from a
                // neighboring triangle, reuse that exact position.
                let p0 = *edge_cache.entry(eid0).or_insert(p0);
                let p1 = *edge_cache.entry(eid1).or_insert(p1);

                // Skip degenerate segments
                if p0.distance(p1) < 1e-12 {
                    continue;
                }
                let v0_idx = add_shared_vert(&mut shared_verts, p0, 1e-10);
                let v1_idx = add_shared_vert(&mut shared_verts, p1, 1e-10);
                if v0_idx != v1_idx {
                    segments.push(IntersectionSegment {
                        tri_a: ai,
                        tri_b: bi,
                        p0,
                        p1,
                        v0_idx,
                        v1_idx,
                    });
                }
            }
        }
    }

    // Step 4: Collect intersection data per triangle
    let mut tri_a_segments: Vec<Vec<usize>> = vec![Vec::new(); mesh_a.triangle_count()];
    let mut tri_b_segments: Vec<Vec<usize>> = vec![Vec::new(); mesh_b.triangle_count()];

    for (si, seg) in segments.iter().enumerate() {
        tri_a_segments[seg.tri_a as usize].push(si);
        tri_b_segments[seg.tri_b as usize].push(si);
    }

    // Step 5: Re-triangulate affected triangles
    let (result_a, origin_a, boundary_a) =
        retriangulate_mesh(mesh_a, &segments, &tri_a_segments, &shared_verts, true);
    let (result_b, origin_b, boundary_b) =
        retriangulate_mesh(mesh_b, &segments, &tri_b_segments, &shared_verts, false);

    CorefinementResult {
        mesh_a: result_a,
        mesh_b: result_b,
        origin_a,
        origin_b,
        on_boundary_a: boundary_a,
        on_boundary_b: boundary_b,
    }
}

/// Add a vertex to the shared pool, merging with existing if within tolerance.
fn add_shared_vert(verts: &mut Vec<Vec3d>, p: Vec3d, tolerance: f64) -> u32 {
    let tol_sq = tolerance * tolerance;
    for (i, &v) in verts.iter().enumerate() {
        if v.distance_sq(p) < tol_sq {
            return i as u32;
        }
    }
    let idx = verts.len() as u32;
    verts.push(p);
    idx
}

/// Re-triangulate a mesh, splitting triangles that are intersected.
fn retriangulate_mesh(
    mesh: &TriMesh,
    segments: &[IntersectionSegment],
    tri_segments: &[Vec<usize>],
    shared_verts: &[Vec3d],
    is_mesh_a: bool,
) -> (TriMesh, Vec<u32>, Vec<bool>) {
    let mut result = TriMesh::new();
    let mut origins: Vec<u32> = Vec::new();
    let mut boundaries: Vec<bool> = Vec::new();

    for ti in 0..mesh.triangle_count() {
        let segs = &tri_segments[ti];
        let (v0, v1, v2) = mesh.triangle_vertices(ti);

        if segs.is_empty() {
            // No intersection: copy triangle as-is
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
            origins.push(ti as u32);
            boundaries.push(false);
        } else {
            // Triangle is intersected: re-triangulate using CDT
            let new_tris =
                retriangulate_triangle(v0, v1, v2, segs, segments, shared_verts, is_mesh_a);
            for [a, b, c] in new_tris {
                let ia = result.add_vertex(a);
                let ib = result.add_vertex(b);
                let ic = result.add_vertex(c);
                result.add_triangle(ia, ib, ic);
                origins.push(ti as u32);
                boundaries.push(true);
            }
        }
    }

    (result, origins, boundaries)
}

/// Re-triangulate a single triangle that has intersection segments.
/// Projects to 2D, runs CDT, and maps back to 3D.
fn retriangulate_triangle(
    v0: Vec3d,
    v1: Vec3d,
    v2: Vec3d,
    seg_indices: &[usize],
    segments: &[IntersectionSegment],
    _shared_verts: &[Vec3d],
    _is_mesh_a: bool,
) -> Vec<[Vec3d; 3]> {
    let normal = (v1 - v0).cross(v2 - v0);
    let normal_len = normal.length();
    if normal_len < 1e-15 {
        return vec![[v0, v1, v2]]; // degenerate triangle, return as-is
    }

    // Choose projection plane: drop the axis most aligned with the normal
    let ax = normal.x.abs();
    let ay = normal.y.abs();
    let az = normal.z.abs();
    let (proj_i, proj_j) = if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    };

    let project = |v: Vec3d| -> (f64, f64) {
        let coords = [v.x, v.y, v.z];
        (coords[proj_i], coords[proj_j])
    };

    // Collect all points: triangle vertices + intersection points
    let mut all_points_3d: Vec<Vec3d> = vec![v0, v1, v2];
    let mut constraint_pairs: Vec<(usize, usize)> = Vec::new();

    for &si in seg_indices {
        let seg = &segments[si];
        // Use canonical positions from the shared vertex pool so that
        // both meshes get identical coordinates for the same intersection point.
        let pts = [
            _shared_verts[seg.v0_idx as usize],
            _shared_verts[seg.v1_idx as usize],
        ];
        let mut pt_indices = [0usize; 2];

        for k in 0..2 {
            let p = pts[k];
            // Check if this point is very close to an existing point
            let mut found = None;
            for (i, &existing) in all_points_3d.iter().enumerate() {
                if existing.distance(p) < 1e-10 {
                    found = Some(i);
                    break;
                }
            }
            pt_indices[k] = match found {
                Some(i) => i,
                None => {
                    let i = all_points_3d.len();
                    all_points_3d.push(p);
                    i
                }
            };
        }

        if pt_indices[0] != pt_indices[1] {
            constraint_pairs.push((pt_indices[0], pt_indices[1]));
        }
    }

    // If we only have the original 3 points and no interior points, return the original triangle
    if all_points_3d.len() == 3 && constraint_pairs.is_empty() {
        return vec![[v0, v1, v2]];
    }

    // Project all points to 2D
    let pts_2d: Vec<(f64, f64)> = all_points_3d.iter().map(|&v| project(v)).collect();

    // Find bounds for CDT
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for &(x, y) in &pts_2d {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    // Run CDT
    let mut cdt = CDT::new(Point2 { x: min_x, y: min_y }, Point2 { x: max_x, y: max_y });

    // Insert all points
    let mut cdt_indices: Vec<u32> = Vec::new();
    for &(x, y) in &pts_2d {
        cdt_indices.push(cdt.insert_point(x, y));
    }

    // Add constraint edges
    for &(a, b) in &constraint_pairs {
        cdt.add_constraint(cdt_indices[a], cdt_indices[b]);
    }

    // Add triangle boundary edges as constraints.
    // If intersection points lie on a boundary edge, split the constraint
    // at those points. Without this, the CDT marks the full edge as constrained
    // and cannot properly split it, producing degenerate triangles.
    for &(e0, e1) in &[(0usize, 1usize), (1, 2), (2, 0)] {
        add_split_boundary_constraint(&mut cdt, &cdt_indices, &pts_2d, e0, e1);
    }

    cdt.finalize();

    // Get result triangles
    let cdt_tris = cdt.get_triangles();

    // Filter: only keep triangles inside the original triangle
    let (p0_2d, p1_2d, p2_2d) = (pts_2d[0], pts_2d[1], pts_2d[2]);
    let tri_orient = orient_2d_tuple(p0_2d, p1_2d, p2_2d);

    let mut result: Vec<[Vec3d; 3]> = Vec::new();
    for &[a, b, c] in &cdt_tris {
        if a as usize >= all_points_3d.len()
            || b as usize >= all_points_3d.len()
            || c as usize >= all_points_3d.len()
        {
            continue;
        }
        // Check if centroid of this sub-triangle is inside the original triangle
        let pa = pts_2d[a as usize];
        let pb = pts_2d[b as usize];
        let pc = pts_2d[c as usize];

        // Skip degenerate (zero-area) triangles — these arise when 3 collinear
        // intersection points form a triangle. Their centroid lies on the boundary,
        // passing the point_in_triangle test, but they create non-manifold edges.
        let sub_area = orient_2d_tuple(pa, pb, pc).abs();
        if sub_area < 1e-14 {
            continue;
        }

        let centroid = ((pa.0 + pb.0 + pc.0) / 3.0, (pa.1 + pb.1 + pc.1) / 3.0);

        if point_in_triangle_2d(centroid, p0_2d, p1_2d, p2_2d) {
            let va = all_points_3d[a as usize];
            let vb = all_points_3d[b as usize];
            let vc = all_points_3d[c as usize];

            // Ensure winding matches original triangle
            let sub_orient = orient_2d_tuple(pa, pb, pc);
            if (tri_orient > 0.0) == (sub_orient > 0.0) {
                result.push([va, vb, vc]);
            } else {
                result.push([va, vc, vb]);
            }
        }
    }

    // If CDT produced nothing valid, fall back to original triangle
    if result.is_empty() {
        result.push([v0, v1, v2]);
    }

    result
}

/// Add a boundary edge constraint, splitting it at any interior points that lie on it.
/// This prevents the CDT from marking the full edge as a single constraint when
/// intersection points split it.
fn add_split_boundary_constraint(
    cdt: &mut CDT,
    cdt_indices: &[u32],
    pts_2d: &[(f64, f64)],
    e0: usize,
    e1: usize,
) {
    let a = pts_2d[e0];
    let b = pts_2d[e1];
    let ab_len_sq = (b.0 - a.0) * (b.0 - a.0) + (b.1 - a.1) * (b.1 - a.1);
    if ab_len_sq < 1e-30 {
        return;
    }

    // Find all points (beyond the triangle vertices 0,1,2) that lie on edge e0→e1
    let mut on_edge: Vec<(usize, f64)> = Vec::new(); // (point_index, t_parameter)
    for i in 3..pts_2d.len() {
        if i == e0 || i == e1 {
            continue;
        }
        let p = pts_2d[i];
        let ap = (p.0 - a.0, p.1 - a.1);
        let t = (ap.0 * (b.0 - a.0) + ap.1 * (b.1 - a.1)) / ab_len_sq;
        if t <= 1e-10 || t >= 1.0 - 1e-10 {
            continue; // at or beyond endpoints
        }
        let proj = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        let dist_sq = (p.0 - proj.0) * (p.0 - proj.0) + (p.1 - proj.1) * (p.1 - proj.1);
        if dist_sq < 1e-16 {
            on_edge.push((i, t));
        }
    }

    if on_edge.is_empty() {
        // No splits needed
        cdt.add_constraint(cdt_indices[e0], cdt_indices[e1]);
    } else {
        // Sort by parameter t along the edge
        on_edge.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        // Add chain of constraints: e0→p1→p2→...→e1
        let mut prev = e0;
        for &(pi, _) in &on_edge {
            cdt.add_constraint(cdt_indices[prev], cdt_indices[pi]);
            prev = pi;
        }
        cdt.add_constraint(cdt_indices[prev], cdt_indices[e1]);
    }
}

/// Compute the edges of the intersection polygon of two coplanar triangles.
///
/// Uses Sutherland-Hodgman clipping: clips triangle A against each halfplane
/// defined by the edges of triangle B. Returns the edges of the resulting
/// polygon as 3D line segments. If the triangles don't overlap, returns empty.
fn coplanar_intersection_edges(
    a0: Vec3d,
    a1: Vec3d,
    a2: Vec3d,
    b0: Vec3d,
    b1: Vec3d,
    b2: Vec3d,
) -> Vec<(Vec3d, Vec3d)> {
    // Project to 2D (drop the axis most aligned with the shared normal)
    let normal = (a1 - a0).cross(a2 - a0);
    let ax = normal.x.abs();
    let ay = normal.y.abs();
    let az = normal.z.abs();
    let (pi, pj) = if ax >= ay && ax >= az {
        (1, 2)
    } else if ay >= az {
        (0, 2)
    } else {
        (0, 1)
    };
    let project = |v: Vec3d| -> [f64; 2] {
        let c = [v.x, v.y, v.z];
        [c[pi], c[pj]]
    };

    // Sutherland-Hodgman: start with triangle A's vertices as polygon,
    // clip against each edge of B.
    let mut polygon: Vec<Vec3d> = vec![a0, a1, a2];
    let clip_edges = [(b0, b1), (b1, b2), (b2, b0)];

    // Determine winding of B in 2D to know which side is "inside"
    let bp0 = project(b0);
    let bp1 = project(b1);
    let bp2 = project(b2);
    let b_orient = (bp1[0] - bp0[0]) * (bp2[1] - bp0[1]) - (bp1[1] - bp0[1]) * (bp2[0] - bp0[0]);
    // sign > 0 means CCW; for each clip edge, "inside" is the left side

    for &(ce0, ce1) in &clip_edges {
        if polygon.is_empty() {
            break;
        }
        let e0 = project(ce0);
        let e1 = project(ce1);
        let mut output = Vec::new();

        for i in 0..polygon.len() {
            let curr = polygon[i];
            let prev = polygon[(i + polygon.len() - 1) % polygon.len()];
            let curr_p = project(curr);
            let prev_p = project(prev);

            // Signed distance from the clip edge (positive = inside for CCW B)
            let d_curr =
                (e1[0] - e0[0]) * (curr_p[1] - e0[1]) - (e1[1] - e0[1]) * (curr_p[0] - e0[0]);
            let d_prev =
                (e1[0] - e0[0]) * (prev_p[1] - e0[1]) - (e1[1] - e0[1]) * (prev_p[0] - e0[0]);

            // Adjust sign based on B's winding
            let d_curr = d_curr * b_orient.signum();
            let d_prev = d_prev * b_orient.signum();

            if d_curr >= 0.0 {
                // Current is inside
                if d_prev < 0.0 {
                    // Previous was outside: add intersection
                    let t = d_prev / (d_prev - d_curr);
                    output.push(prev.lerp(curr, t));
                }
                output.push(curr);
            } else if d_prev >= 0.0 {
                // Current is outside, previous was inside: add intersection
                let t = d_prev / (d_prev - d_curr);
                output.push(prev.lerp(curr, t));
            }
        }
        polygon = output;
    }

    if polygon.len() < 3 {
        return Vec::new(); // Degenerate or no intersection
    }

    // Return edges of the intersection polygon
    let mut edges = Vec::with_capacity(polygon.len());
    for i in 0..polygon.len() {
        let j = (i + 1) % polygon.len();
        edges.push((polygon[i], polygon[j]));
    }
    edges
}

fn orient_2d_tuple(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn point_in_triangle_2d(p: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> bool {
    let d1 = orient_2d_tuple(a, b, p);
    let d2 = orient_2d_tuple(b, c, p);
    let d3 = orient_2d_tuple(c, a, p);

    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::{dvec3, Mat4d};
    use makepad_csg_mesh::mesh::make_unit_cube;

    #[test]
    fn test_corefine_non_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));

        let result = corefine(&a, &b);
        // Non-overlapping: meshes should be unchanged
        assert_eq!(result.mesh_a.triangle_count(), 12);
        assert_eq!(result.mesh_b.triangle_count(), 12);
    }

    #[test]
    fn test_corefine_overlapping_cubes() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let result = corefine(&a, &b);
        // Overlapping: meshes should have more triangles
        assert!(
            result.mesh_a.triangle_count() >= 12,
            "mesh A should have >= 12 triangles, got {}",
            result.mesh_a.triangle_count()
        );
        assert!(
            result.mesh_b.triangle_count() >= 12,
            "mesh B should have >= 12 triangles, got {}",
            result.mesh_b.triangle_count()
        );

        // Origin tracking should be valid
        assert_eq!(result.origin_a.len(), result.mesh_a.triangle_count());
        assert_eq!(result.origin_b.len(), result.mesh_b.triangle_count());
    }

    #[test]
    fn test_corefine_preserves_volume() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let vol_a_before = makepad_csg_mesh::volume::mesh_volume(&a);
        let vol_b_before = makepad_csg_mesh::volume::mesh_volume(&b);

        let result = corefine(&a, &b);

        let vol_a_after = makepad_csg_mesh::volume::mesh_volume(&result.mesh_a);
        let vol_b_after = makepad_csg_mesh::volume::mesh_volume(&result.mesh_b);

        // Corefinement should preserve volume (within tolerance)
        assert!(
            (vol_a_before - vol_a_after).abs() < 0.1,
            "mesh A volume changed: {} -> {}",
            vol_a_before,
            vol_a_after
        );
        assert!(
            (vol_b_before - vol_b_after).abs() < 0.1,
            "mesh B volume changed: {} -> {}",
            vol_b_before,
            vol_b_after
        );
    }
}
