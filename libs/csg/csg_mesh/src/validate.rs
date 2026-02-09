// Mesh validation: manifold checks, orientation consistency, edge analysis.

use crate::mesh::TriMesh;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct MeshReport {
    pub is_closed: bool,
    pub is_manifold: bool,
    pub is_consistently_oriented: bool,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub degenerate_triangles: usize,
    pub total_edges: usize,
}

/// An oriented edge: (from, to) vertex indices.
type Edge = (u32, u32);

/// Validate mesh topology.
pub fn validate_mesh(mesh: &TriMesh) -> MeshReport {
    // Count how many times each directed edge appears
    let mut edge_count: HashMap<Edge, u32> = HashMap::new();

    let mut degenerate_triangles = 0;

    for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate() {
        if a == b || b == c || c == a {
            degenerate_triangles += 1;
            continue;
        }
        // Check for zero-area triangles
        if mesh.triangle_area(ti) < 1e-15 {
            degenerate_triangles += 1;
        }

        *edge_count.entry((a, b)).or_insert(0) += 1;
        *edge_count.entry((b, c)).or_insert(0) += 1;
        *edge_count.entry((c, a)).or_insert(0) += 1;
    }

    // For each undirected edge {u,v}, check:
    // - In a closed manifold, edge (u,v) appears once and (v,u) appears once
    // - Boundary edge: only one direction present
    // - Non-manifold: more than 2 total appearances
    let mut undirected_edges: HashMap<(u32, u32), (u32, u32)> = HashMap::new(); // (forward_count, reverse_count)
    for (&(a, b), &count) in &edge_count {
        let key = if a < b { (a, b) } else { (b, a) };
        let entry = undirected_edges.entry(key).or_insert((0, 0));
        if a < b {
            entry.0 += count;
        } else {
            entry.1 += count;
        }
    }

    let mut boundary_edges = 0;
    let mut non_manifold_edges = 0;
    let mut consistently_oriented = true;

    for &(fwd, rev) in undirected_edges.values() {
        let total = fwd + rev;
        if total == 1 {
            boundary_edges += 1;
        } else if total > 2 {
            non_manifold_edges += 1;
        }
        // Consistent orientation: each undirected edge should have exactly
        // one forward and one reverse directed edge.
        if total == 2 && (fwd != 1 || rev != 1) {
            consistently_oriented = false;
        }
    }

    let is_closed = boundary_edges == 0;
    let is_manifold = non_manifold_edges == 0 && boundary_edges == 0;

    MeshReport {
        is_closed,
        is_manifold,
        is_consistently_oriented: consistently_oriented,
        boundary_edges,
        non_manifold_edges,
        degenerate_triangles,
        total_edges: undirected_edges.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_unit_cube;

    #[test]
    fn test_cube_is_valid() {
        let cube = make_unit_cube();
        let report = validate_mesh(&cube);
        assert!(report.is_closed, "cube should be closed");
        assert!(report.is_manifold, "cube should be manifold");
        assert!(
            report.is_consistently_oriented,
            "cube should be consistently oriented"
        );
        assert_eq!(report.boundary_edges, 0);
        assert_eq!(report.non_manifold_edges, 0);
        assert_eq!(report.degenerate_triangles, 0);
        // Cube has 12 edges
        assert_eq!(report.total_edges, 18); // 12 original edges + 6 diagonal edges from triangulation
    }

    #[test]
    fn test_open_mesh() {
        // Remove one triangle to create a hole
        let mut cube = make_unit_cube();
        cube.triangles.pop();
        let report = validate_mesh(&cube);
        assert!(
            !report.is_closed,
            "mesh with missing triangle should not be closed"
        );
        assert!(
            !report.is_manifold,
            "mesh with missing triangle should not be manifold"
        );
        assert!(report.boundary_edges > 0);
    }

    #[test]
    fn test_flipped_cube_oriented() {
        // Flipping all normals should still be consistently oriented
        let mut cube = make_unit_cube();
        cube.flip_normals();
        let report = validate_mesh(&cube);
        assert!(report.is_consistently_oriented);
        assert!(report.is_closed);
    }
}
