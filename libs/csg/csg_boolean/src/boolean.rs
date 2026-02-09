// Corefinement-based boolean operations.
//
// - Preserves original triangles away from the intersection
// - Produces high-quality mesh output
// - Scales as O(n log n + k) where k = intersection edges
//
// Algorithm:
// 1. Corefine both meshes (split along intersection curve)
// 2. Classify each triangle as inside/outside the other mesh
// 3. Select faces based on the boolean operation

use crate::classify::{classify_triangles, point_inside_mesh, TriLocation};
use crate::corefine::corefine;
use makepad_csg_math::Vec3d;
use makepad_csg_mesh::mesh::TriMesh;

/// Boolean operation type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

/// Perform a boolean operation on two triangle meshes.
pub fn mesh_boolean(mesh_a: &TriMesh, mesh_b: &TriMesh, op: BoolOp) -> TriMesh {
    // Step 1: Corefine both meshes
    let coref = corefine(mesh_a, mesh_b);

    // Step 2: Classify triangles
    let class_a = classify_triangles(&coref.mesh_a, &coref.mesh_b, &coref.on_boundary_a);
    let class_b = classify_triangles(&coref.mesh_b, &coref.mesh_a, &coref.on_boundary_b);

    // Step 3: Select faces based on operation
    //
    // OnBoundary faces (where both +/- normal sides are inside the other mesh)
    // are kept from mesh A only, dropped from mesh B.
    //
    // "Surface-coplanar" boundary faces: these are boundary faces classified
    // as Inside where the face lies on the surface (not interior) of the other
    // mesh. Both meshes contribute them, causing double-counting. We detect
    // these by checking if +normal is outside the other mesh (surface face)
    // vs inside (interior face). Surface-coplanar Inside faces from B are dropped.
    let mut result = TriMesh::new();

    // From mesh A: A has priority for surface-coplanar faces
    for ti in 0..coref.mesh_a.triangle_count() {
        let c = class_a[ti];
        let mut keep = match op {
            BoolOp::Union => c == TriLocation::Outside,
            BoolOp::Difference => c == TriLocation::Outside,
            BoolOp::Intersection => c == TriLocation::Inside || c == TriLocation::OnBoundary,
        };

        // For union and intersection: boundary faces classified Inside that
        // are surface-coplanar (+normal outside the other mesh) need to be
        // kept from A. These faces lie on the shared surface and A has
        // priority (B's duplicates are dropped below).
        // For difference A-B: these faces are cut away by B, not kept.
        if !keep
            && op != BoolOp::Difference
            && coref.on_boundary_a[ti]
            && (c == TriLocation::Inside || c == TriLocation::OnBoundary)
        {
            let centroid = {
                let (v0, v1, v2) = coref.mesh_a.triangle_vertices(ti);
                (v0 + v1 + v2) / 3.0
            };
            let normal = coref.mesh_a.triangle_normal(ti);
            let eps = 1e-6;
            let outside_point = centroid + normal * eps;
            if !point_inside_mesh(outside_point, &coref.mesh_b) {
                // Surface-coplanar: this face is on B's surface.
                // Keep it from A (B's duplicate will be dropped).
                keep = true;
            }
        }

        if keep {
            let (v0, v1, v2) = coref.mesh_a.triangle_vertices(ti);
            let a = result.add_vertex(v0);
            let b = result.add_vertex(v1);
            let c = result.add_vertex(v2);
            result.add_triangle(a, b, c);
        }
    }

    // From mesh B: keep qualifying faces, drop surface-coplanar duplicates
    for ti in 0..coref.mesh_b.triangle_count() {
        let c = class_b[ti];

        // Base selection
        let keep = match op {
            BoolOp::Union => c == TriLocation::Outside,
            BoolOp::Intersection => c == TriLocation::Inside || c == TriLocation::OnBoundary,
            BoolOp::Difference => c == TriLocation::Inside || c == TriLocation::OnBoundary,
        };

        if !keep {
            continue;
        }

        // For boundary Inside faces from B: check if this is a surface-coplanar
        // face. Surface-coplanar means +normal is outside the other mesh (the
        // face is on A's surface). A already contributes these, so skip B's copy.
        //
        // OnBoundary faces (both sides inside A) are NOT surface-coplanar — they
        // are genuinely inside A's volume. These are kept for intersection/difference.
        if coref.on_boundary_b[ti] && c == TriLocation::Inside {
            let centroid = {
                let (v0, v1, v2) = coref.mesh_b.triangle_vertices(ti);
                (v0 + v1 + v2) / 3.0
            };
            let normal = coref.mesh_b.triangle_normal(ti);
            let eps = 1e-6;
            let outside_point = centroid + normal * eps;
            if !point_inside_mesh(outside_point, &coref.mesh_a) {
                // Surface-coplanar: A already has this face.
                continue;
            }
        }

        // For union: also drop OnBoundary faces (they're interior to the union)
        if op == BoolOp::Union && c == TriLocation::OnBoundary {
            continue;
        }

        let (v0, v1, v2) = coref.mesh_b.triangle_vertices(ti);
        match op {
            BoolOp::Difference => {
                let a = result.add_vertex(v0);
                let b = result.add_vertex(v1);
                let c = result.add_vertex(v2);
                result.add_triangle(a, c, b); // reversed winding
            }
            _ => {
                let a = result.add_vertex(v0);
                let b = result.add_vertex(v1);
                let c = result.add_vertex(v2);
                result.add_triangle(a, b, c);
            }
        }
    }

    // Weld near-coincident vertices from independent intersection computations.
    result.weld_vertices(1e-4);

    // Fix T-junctions iteratively.
    for _ in 0..5 {
        let before = result.triangles.len();
        fix_mesh_t_junctions(&mut result, 1e-4);
        if result.triangles.len() == before {
            break;
        }
        result.weld_vertices(1e-4);
    }

    // Final cleanup: remove sliver triangles from floating-point imprecision.
    // Uses minimum altitude criterion: h_min = 2*area / max_edge.
    // Slivers from cascaded booleans have altitudes ~1e-6 (e.g., area=2e-5,
    // base=10 → altitude=4e-6). Real thin triangles from small overlaps or
    // slight radius differences have altitudes >= ~1e-3. Threshold 1e-4 is
    // well within this gap.
    remove_degenerate_triangles(&mut result, 1e-4);
    result.weld_vertices(1e-4);

    result
}

/// Union of two meshes using corefinement.
pub fn union(mesh_a: &TriMesh, mesh_b: &TriMesh) -> TriMesh {
    mesh_boolean(mesh_a, mesh_b, BoolOp::Union)
}

/// Difference (A - B) using corefinement.
pub fn difference(mesh_a: &TriMesh, mesh_b: &TriMesh) -> TriMesh {
    mesh_boolean(mesh_a, mesh_b, BoolOp::Difference)
}

/// Intersection (A & B) using corefinement.
pub fn intersection(mesh_a: &TriMesh, mesh_b: &TriMesh) -> TriMesh {
    mesh_boolean(mesh_a, mesh_b, BoolOp::Intersection)
}

// --- T-junction resolution for triangle meshes ---
//
// After corefinement + face selection, the result mesh may have T-junctions:
// a vertex from one mesh's CDT retriangulation lies on an edge of a triangle
// from the other mesh. This creates boundary edges (gaps).
//
// Fix: for each vertex in the mesh, check if it lies on any triangle edge
// (not at an endpoint). If so, split that triangle into two by inserting
// the vertex.

/// Fix T-junctions in a welded mesh by splitting triangles at vertices
/// that lie on their edges.
fn fix_mesh_t_junctions(mesh: &mut TriMesh, tol: f64) {
    // May need multiple passes since splitting can create new T-junctions.
    // Each pass can expose new T-junctions where a split triangle edge
    // now has a vertex on it.
    for _ in 0..10 {
        if !fix_t_junctions_pass(mesh, tol) {
            break;
        }
    }
}

/// Single pass of T-junction fixing. Returns true if any splits were made.
///
/// Uses a spatial grid to avoid O(n*m) brute-force vertex-edge checks.
/// Each vertex is inserted into a grid cell; for each triangle edge, only
/// vertices in nearby cells are tested.
fn fix_t_junctions_pass(mesh: &mut TriMesh, tol: f64) -> bool {
    let num_verts = mesh.vertices.len();
    let num_tris = mesh.triangles.len();
    if num_tris == 0 || num_verts == 0 {
        return false;
    }

    // Choose cell size based on mesh extent so we get ~10-50 vertices per cell,
    // not based on tolerance (which would create a huge grid for tiny tolerances).
    let bbox = mesh.bounding_box();
    let extent = (bbox.max - bbox.min).length();
    // Target: cube root of num_verts cells per axis gives ~1 vert/cell.
    // Use a bit larger to reduce misses.
    let cells_per_axis = (num_verts as f64).cbrt().max(4.0);
    let cell = (extent / cells_per_axis).max(tol * 2.0);
    let inv_cell = 1.0 / cell;

    let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<u32>> =
        std::collections::HashMap::new();
    for (i, v) in mesh.vertices.iter().enumerate() {
        let cx = (v.x * inv_cell).floor() as i64;
        let cy = (v.y * inv_cell).floor() as i64;
        let cz = (v.z * inv_cell).floor() as i64;
        grid.entry((cx, cy, cz)).or_default().push(i as u32);
    }

    // For each triangle edge, query nearby vertices from the grid.
    let mut new_triangles: Vec<[u32; 3]> = Vec::new();
    let mut removed = vec![false; num_tris];
    let mut any_split = false;

    for ti in 0..num_tris {
        if removed[ti] {
            continue;
        }
        let [ia, ib, ic] = mesh.triangles[ti];
        let va = mesh.vertices[ia as usize];
        let vb = mesh.vertices[ib as usize];
        let vc = mesh.vertices[ic as usize];

        let edges = [
            (ia, ib, ic, va, vb),
            (ib, ic, ia, vb, vc),
            (ic, ia, ib, vc, va),
        ];

        let mut split_found = false;
        for &(e0, e1, opp, v0, v1) in &edges {
            if split_found {
                break;
            }
            // Compute AABB of edge expanded by tolerance, query grid cells
            let min_x = v0.x.min(v1.x) - tol;
            let min_y = v0.y.min(v1.y) - tol;
            let min_z = v0.z.min(v1.z) - tol;
            let max_x = v0.x.max(v1.x) + tol;
            let max_y = v0.y.max(v1.y) + tol;
            let max_z = v0.z.max(v1.z) + tol;

            let c_min_x = (min_x * inv_cell).floor() as i64;
            let c_min_y = (min_y * inv_cell).floor() as i64;
            let c_min_z = (min_z * inv_cell).floor() as i64;
            let c_max_x = (max_x * inv_cell).floor() as i64;
            let c_max_y = (max_y * inv_cell).floor() as i64;
            let c_max_z = (max_z * inv_cell).floor() as i64;

            'edge_search: for gx in c_min_x..=c_max_x {
                for gy in c_min_y..=c_max_y {
                    for gz in c_min_z..=c_max_z {
                        if let Some(bucket) = grid.get(&(gx, gy, gz)) {
                            for &vi32 in bucket {
                                if vi32 == e0 || vi32 == e1 || vi32 == opp {
                                    continue;
                                }
                                if point_on_edge(mesh.vertices[vi32 as usize], v0, v1, tol) {
                                    removed[ti] = true;
                                    new_triangles.push([e0, vi32, opp]);
                                    new_triangles.push([vi32, e1, opp]);
                                    split_found = true;
                                    any_split = true;
                                    break 'edge_search;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if !any_split {
        return false;
    }

    let old_tris = std::mem::take(&mut mesh.triangles);
    for (i, tri) in old_tris.into_iter().enumerate() {
        if !removed[i] {
            mesh.triangles.push(tri);
        }
    }
    mesh.triangles.extend(new_triangles);
    true
}

/// Check if point p lies on the line segment a->b (not at endpoints), within tolerance.
fn point_on_edge(p: Vec3d, a: Vec3d, b: Vec3d, tol: f64) -> bool {
    let ab = b - a;
    let ab_len_sq = ab.length_sq();
    if ab_len_sq < 1e-30 {
        return false;
    }

    // Skip if p is at either endpoint
    if p.distance(a) < tol || p.distance(b) < tol {
        return false;
    }

    // Project p onto line a->b
    let ap = p - a;
    let t = ap.dot(ab) / ab_len_sq;
    if t <= 0.0 || t >= 1.0 {
        return false;
    }

    // Check distance from line
    let proj = a + ab * t;
    (p - proj).length() < tol
}

/// Remove degenerate/sliver triangles produced by floating-point imprecision.
///
/// Uses the minimum altitude criterion: `h_min = 2 * area / max_edge_length`.
/// Slivers from cascaded boolean imprecision typically have altitudes ~1e-6
/// (e.g., area=2e-5 with base=10), while real thin triangles from overlap
/// regions have altitudes on the order of the overlap (e.g., ~0.01).
/// A threshold of 1e-3 cleanly separates these.
fn remove_degenerate_triangles(mesh: &mut TriMesh, min_altitude: f64) {
    let mut keep = Vec::with_capacity(mesh.triangles.len());
    for &[a, b, c] in &mesh.triangles {
        if a == b || b == c || c == a {
            continue;
        }
        let va = mesh.vertices[a as usize];
        let vb = mesh.vertices[b as usize];
        let vc = mesh.vertices[c as usize];
        // Compute area via cross product
        let cross = (vb - va).cross(vc - va);
        let area2 = cross.length(); // = 2 * area
                                    // Find longest edge
        let lab = (vb - va).length();
        let lbc = (vc - vb).length();
        let lca = (va - vc).length();
        let max_edge = lab.max(lbc).max(lca);
        if max_edge < 1e-15 {
            continue; // zero-size triangle
        }
        // Minimum altitude = 2 * area / max_edge = area2 / max_edge
        let h_min = area2 / max_edge;
        if h_min < min_altitude {
            continue;
        }
        keep.push([a, b, c]);
    }
    mesh.triangles = keep;
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::{dvec3, Mat4d};
    use makepad_csg_mesh::mesh::make_unit_cube;
    use makepad_csg_mesh::volume::mesh_volume;

    #[test]
    fn test_union_non_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));

        let result = union(&a, &b);
        let vol = mesh_volume(&result);
        assert!(
            (vol - 2.0).abs() < 0.2,
            "non-overlapping union volume: {}",
            vol
        );
    }

    #[test]
    fn test_union_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let result = union(&a, &b);
        let vol = mesh_volume(&result);
        // Expected: 2.0 - 0.5 = 1.5
        assert!(
            vol > 1.0 && vol < 2.0,
            "overlapping union volume should be ~1.5, got {}",
            vol
        );
    }

    #[test]
    fn test_difference_non_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));

        let result = difference(&a, &b);
        let vol = mesh_volume(&result);
        assert!(
            (vol - 1.0).abs() < 0.2,
            "non-overlapping difference volume: {}",
            vol
        );
    }

    #[test]
    fn test_difference_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let result = difference(&a, &b);
        let vol = mesh_volume(&result);
        // Expected: 1.0 - 0.5 = 0.5
        assert!(
            vol > 0.0 && vol < 1.0,
            "overlapping difference volume should be ~0.5, got {}",
            vol
        );
    }

    #[test]
    fn test_intersection_non_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(5.0, 0.0, 0.0)));

        let result = intersection(&a, &b);
        // Non-overlapping intersection should be empty or nearly empty
        let vol = mesh_volume(&result);
        assert!(
            vol.abs() < 0.1,
            "non-overlapping intersection volume should be ~0, got {}",
            vol
        );
    }

    #[test]
    fn test_intersection_overlapping() {
        let a = make_unit_cube();
        let mut b = make_unit_cube();
        b.transform(Mat4d::translation(dvec3(0.5, 0.0, 0.0)));

        let result = intersection(&a, &b);
        let vol = mesh_volume(&result);
        // Expected: 0.5 x 1.0 x 1.0 = 0.5
        assert!(
            vol > 0.0,
            "overlapping intersection volume should be positive, got {}",
            vol
        );
    }
}
