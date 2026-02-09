// CSG boolean operations using BSP trees.
//
// Based on the csg.js algorithm (Laidlaw/Trumbore/Hughes 1986, Naylor 1990):
//   Union:        clip A to B, clip B to A, merge
//   Difference:   invert A, union with B, invert result
//   Intersection: invert both, union, invert result

use crate::bsp_node::BspNode;
use crate::polygon::Polygon;
use makepad_csg_math::Vec3d;

/// Compute the union of two polygon sets (A | B).
/// Result contains the surfaces of both solids minus interior overlaps.
pub fn union(a_polys: Vec<Polygon>, b_polys: Vec<Polygon>) -> Vec<Polygon> {
    if a_polys.is_empty() {
        return b_polys;
    }
    if b_polys.is_empty() {
        return a_polys;
    }

    let mut a = match BspNode::build(a_polys) {
        Some(n) => n,
        None => return b_polys,
    };
    let mut b = match BspNode::build(b_polys) {
        Some(n) => n,
        None => return a.all_polygons(),
    };

    // Remove A polygons inside B
    a.clip_to(&b);
    // Remove B polygons inside A
    b.clip_to(&a);
    // Remove B polygons inside A (inverted pass to handle coplanar)
    b.invert();
    b.clip_to(&a);
    b.invert();

    // Fix T-junctions before merging
    let a_polys = a.all_polygons();
    let b_polys = b.all_polygons();
    let (a_fixed, b_fixed) = fix_t_junctions(a_polys, b_polys);

    let mut result = a_fixed;
    result.extend(b_fixed);
    result
}

/// Compute the difference of two polygon sets (A - B).
/// Result contains A minus the volume of B.
pub fn difference(a_polys: Vec<Polygon>, b_polys: Vec<Polygon>) -> Vec<Polygon> {
    if a_polys.is_empty() {
        return Vec::new();
    }
    if b_polys.is_empty() {
        return a_polys;
    }

    let mut a = match BspNode::build(a_polys) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let mut b = match BspNode::build(b_polys) {
        Some(n) => n,
        None => return a.all_polygons(),
    };

    // A - B = ~(~A | B)
    a.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    b.invert();
    b.clip_to(&a);
    b.invert();

    // Fix T-junctions before merging
    let a_polys = a.all_polygons();
    let b_polys = b.all_polygons();
    let (mut a_fixed, b_fixed) = fix_t_junctions(a_polys, b_polys);
    a_fixed.extend(b_fixed);

    // Invert the merged result
    for p in &mut a_fixed {
        p.flip();
    }
    a_fixed
}

/// Compute the intersection of two polygon sets (A & B).
/// Result contains only the volume where both solids overlap.
pub fn intersection(a_polys: Vec<Polygon>, b_polys: Vec<Polygon>) -> Vec<Polygon> {
    if a_polys.is_empty() || b_polys.is_empty() {
        return Vec::new();
    }

    let mut a = match BspNode::build(a_polys) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let mut b = match BspNode::build(b_polys) {
        Some(n) => n,
        None => return Vec::new(),
    };

    // A & B = ~(~A | ~B)
    a.invert();
    b.clip_to(&a);
    b.invert();
    a.clip_to(&b);
    b.clip_to(&a);

    // Fix T-junctions before merging
    let a_polys = a.all_polygons();
    let b_polys = b.all_polygons();
    let (mut a_fixed, b_fixed) = fix_t_junctions(a_polys, b_polys);
    a_fixed.extend(b_fixed);

    // Invert the merged result
    for p in &mut a_fixed {
        p.flip();
    }
    a_fixed
}

// --- T-junction resolution ---
//
// BSP clipping creates new vertices when splitting polygons against a splitting
// plane. These new vertices often land on edges of the *other* mesh's polygons,
// creating T-junctions: one polygon has a vertex on an edge of an adjacent
// polygon, but the adjacent polygon doesn't have a corresponding vertex there.
// This causes boundary edges (gaps) in the final mesh.
//
// Fix: after clipping both polygon sets, find T-junctions (vertices from one
// set that lie on edges of the other) and insert those vertices into the edges,
// splitting the affected polygons.

const T_JUNCTION_TOL: f64 = 1e-8;

/// Fix T-junctions between two polygon sets (and within each set).
/// Returns the fixed versions of both sets.
fn fix_t_junctions(a_polys: Vec<Polygon>, b_polys: Vec<Polygon>) -> (Vec<Polygon>, Vec<Polygon>) {
    // Collect all unique vertices from both sets combined
    let mut all_verts = collect_unique_vertices(&a_polys, T_JUNCTION_TOL);
    let b_verts = collect_unique_vertices(&b_polys, T_JUNCTION_TOL);
    let tol_sq = T_JUNCTION_TOL * T_JUNCTION_TOL;
    for &v in &b_verts {
        let is_new = all_verts.iter().all(|&e| e.distance_sq(v) > tol_sq);
        if is_new {
            all_verts.push(v);
        }
    }

    // Fix both sets using the combined vertex pool.
    // This resolves inter-set T-junctions (B verts on A edges) AND
    // intra-set T-junctions (e.g., a split vertex from one A polygon
    // landing on an edge of an adjacent A polygon).
    let a_fixed = insert_vertices_on_edges(a_polys, &all_verts, T_JUNCTION_TOL);
    let b_fixed = insert_vertices_on_edges(b_polys, &all_verts, T_JUNCTION_TOL);

    (a_fixed, b_fixed)
}

/// Collect all unique vertices from a polygon set.
fn collect_unique_vertices(polys: &[Polygon], tol: f64) -> Vec<Vec3d> {
    let tol_sq = tol * tol;
    let mut verts: Vec<Vec3d> = Vec::new();
    for p in polys {
        for &v in &p.vertices {
            let is_new = verts.iter().all(|&e| e.distance_sq(v) > tol_sq);
            if is_new {
                verts.push(v);
            }
        }
    }
    verts
}

/// For each polygon, check every edge: if any of the given extra vertices
/// lie on an edge (not at endpoints), insert them to eliminate T-junctions.
fn insert_vertices_on_edges(polys: Vec<Polygon>, extra_verts: &[Vec3d], tol: f64) -> Vec<Polygon> {
    polys
        .into_iter()
        .map(|poly| insert_on_polygon_edges(poly, extra_verts, tol))
        .collect()
}

/// Insert extra vertices on the edges of a single polygon where they create T-junctions.
fn insert_on_polygon_edges(poly: Polygon, extra_verts: &[Vec3d], tol: f64) -> Polygon {
    let n = poly.vertices.len();
    let mut new_verts: Vec<Vec3d> = Vec::new();
    let mut modified = false;

    for i in 0..n {
        let a = poly.vertices[i];
        let b = poly.vertices[(i + 1) % n];
        new_verts.push(a);

        // Find extra vertices on this edge
        let ab = b - a;
        let ab_len_sq = ab.length_sq();
        if ab_len_sq < 1e-30 {
            continue;
        }
        let ab_len = ab_len_sq.sqrt();

        let mut edge_inserts: Vec<(f64, Vec3d)> = Vec::new();
        for &v in extra_verts {
            // Skip if v is near either endpoint
            if v.distance(a) < tol || v.distance(b) < tol {
                continue;
            }

            // Project v onto line a->b
            let av = v - a;
            let t = av.dot(ab) / ab_len_sq;
            if t <= 0.0 || t >= 1.0 {
                continue;
            }

            // Check distance from line
            let proj = a + ab * t;
            let dist = (v - proj).length();
            if dist < tol {
                edge_inserts.push((t, v));
                modified = true;
            }
        }

        // Sort by parameter t and insert
        edge_inserts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (_, v) in edge_inserts {
            new_verts.push(v);
        }
    }

    if !modified {
        return poly;
    }

    Polygon::with_plane(new_verts, poly.plane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{mesh_to_polygons, polygons_to_mesh};
    use makepad_csg_math::dvec3;

    fn cube_polys_at(cx: f64, cy: f64, cz: f64) -> Vec<Polygon> {
        let mut mesh = makepad_csg_mesh::mesh::make_unit_cube();
        mesh.transform(makepad_csg_math::Mat4d::translation(dvec3(cx, cy, cz)));
        mesh_to_polygons(&mesh)
    }

    #[test]
    fn test_union_non_overlapping() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(5.0, 0.0, 0.0);
        let result = union(a, b);
        // Non-overlapping: all polygons should be preserved
        assert_eq!(
            result.len(),
            24,
            "non-overlapping union should have 24 polygons"
        );
    }

    #[test]
    fn test_union_overlapping() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(0.5, 0.0, 0.0);
        let result = union(a, b);
        let mesh = polygons_to_mesh(&result);
        // Should have more than 12 triangles (the interior faces are removed)
        assert!(mesh.triangle_count() > 0, "union should produce triangles");
        // Volume should be greater than 1 cube but less than 2
        let vol = makepad_csg_mesh::volume::mesh_volume(&mesh);
        assert!(
            vol > 1.0 - 0.1,
            "union volume should be > ~1.0, got {}",
            vol
        );
        assert!(
            vol < 2.0 + 0.1,
            "union volume should be < ~2.0, got {}",
            vol
        );
    }

    #[test]
    fn test_difference_non_overlapping() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(5.0, 0.0, 0.0);
        let result = difference(a, b);
        // No overlap: A - B = A
        assert_eq!(
            result.len(),
            12,
            "non-overlapping difference should preserve A"
        );
    }

    #[test]
    fn test_difference_overlapping() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(0.5, 0.0, 0.0);
        let result = difference(a, b);
        let mesh = polygons_to_mesh(&result);
        let vol = makepad_csg_mesh::volume::mesh_volume(&mesh);
        // Volume should be A minus the intersection
        assert!(
            vol > 0.0,
            "difference volume should be positive, got {}",
            vol
        );
        assert!(vol < 1.0, "difference volume should be < 1.0, got {}", vol);
    }

    #[test]
    fn test_intersection_non_overlapping() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(5.0, 0.0, 0.0);
        let result = intersection(a, b);
        // No overlap: intersection should be empty
        assert!(
            result.is_empty(),
            "non-overlapping intersection should be empty, got {} polygons",
            result.len()
        );
    }

    #[test]
    fn test_intersection_overlapping() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(0.5, 0.0, 0.0);
        let result = intersection(a, b);
        let mesh = polygons_to_mesh(&result);
        let vol = makepad_csg_mesh::volume::mesh_volume(&mesh);
        // Intersection volume should be the overlapping region
        assert!(
            vol > 0.0,
            "intersection volume should be positive, got {}",
            vol
        );
        assert!(
            vol < 1.0,
            "intersection volume should be < 1.0, got {}",
            vol
        );
    }

    #[test]
    fn test_union_with_empty() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let result = union(a.clone(), Vec::new());
        assert_eq!(result.len(), a.len());
    }

    #[test]
    fn test_difference_with_empty() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let result = difference(a.clone(), Vec::new());
        assert_eq!(result.len(), a.len());
    }

    #[test]
    fn test_union_identical_cubes() {
        let a = cube_polys_at(0.0, 0.0, 0.0);
        let b = cube_polys_at(0.0, 0.0, 0.0);
        let result = union(a, b);
        let mesh = polygons_to_mesh(&result);
        let vol = makepad_csg_mesh::volume::mesh_volume(&mesh);
        // Union of identical cubes should be ~1.0 volume
        assert!(
            (vol - 1.0).abs() < 0.2,
            "identical cube union volume should be ~1.0, got {}",
            vol
        );
    }
}
