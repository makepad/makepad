// Mesh validation: manifold checks, orientation consistency, edge analysis.

use crate::mesh::TriMesh;
use makepad_csg_math::fast_hash::FastMap;
use makepad_csg_math::Vec3d;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct MeshReport {
    pub is_closed: bool,
    pub is_manifold: bool,
    pub is_consistently_oriented: bool,
    /// Whether tags are empty or have one entry per triangle.
    pub is_tag_length_valid: bool,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub degenerate_triangles: usize,
    pub total_edges: usize,
    /// Pairs of the triangles' vertices closer than [`NEAR_DUPLICATE`] times the mesh's scale (its
    /// largest coordinate, at least 1) but not at one position: two copies of one point left apart.
    /// The edge counts cannot see them, and an edge between them is too short for a flat triangle
    /// (its neighbours are needles of that height, far above [`FLAT_HEIGHT`]).
    pub near_duplicate_pairs: usize,
    /// Pairs at exactly one position that no edge joins: a boolean keeps the fans of a contact point
    /// apart this way on purpose.
    pub coincident_pairs: usize,
}

/// The distance, relative to a mesh's scale, below which two vertices are copies of one point: the
/// boolean's coincidence scale (its plane tolerance).
pub const NEAR_DUPLICATE: f64 = 1e-9;

/// An oriented edge: (from, to) vertex indices.
type Edge = (u32, u32);

/// Count triangle components joined by shared edges (two vertex indices).
/// An empty mesh has no pieces; sharing only a vertex does not join pieces.
pub fn pieces(mesh: &TriMesh) -> usize {
    fn root(parents: &mut [usize], mut i: usize) -> usize {
        while parents[i] != i {
            parents[i] = parents[parents[i]];
            i = parents[i];
        }
        i
    }

    let mut parents: Vec<_> = (0..mesh.triangles.len()).collect();
    let mut sizes = vec![1usize; parents.len()];
    let mut count = parents.len();
    let mut edges = HashMap::new();
    for (i, &[a, b, c]) in mesh.triangles.iter().enumerate() {
        for (a, b) in [(a, b), (b, c), (c, a)] {
            if a == b {
                continue;
            }
            let other = *edges.entry((a.min(b), a.max(b))).or_insert(i);
            let (mut a, mut b) = (root(&mut parents, i), root(&mut parents, other));
            if a != b {
                if sizes[a] < sizes[b] {
                    std::mem::swap(&mut a, &mut b);
                }
                parents[b] = a;
                sizes[a] += sizes[b];
                count -= 1;
            }
        }
    }
    count
}

/// The height over its longest edge (model units) at or below which a triangle is flat: no
/// surface, a degenerate triangle. A mesh boolean's cut keeps every piece above it.
pub const FLAT_HEIGHT: f64 = 1e-10;

/// Whether a triangle is flat: its height over its longest edge is at most [`FLAT_HEIGHT`].
pub fn is_flat(a: Vec3d, b: Vec3d, c: Vec3d) -> bool {
    let twice_area_sq = (b - a).cross(c - a).length_sq();
    let longest_sq = (b - a).length_sq().max((c - b).length_sq()).max((a - c).length_sq());
    twice_area_sq <= FLAT_HEIGHT * FLAT_HEIGHT * longest_sq
}

/// Validate mesh topology.
pub fn validate_mesh(mesh: &TriMesh) -> MeshReport {
    // Count how many times each directed edge appears
    let mut edge_count: HashMap<Edge, u32> = HashMap::new();

    let mut degenerate_triangles = 0;

    for &[a, b, c] in &mesh.triangles {
        if a == b || b == c || c == a {
            degenerate_triangles += 1;
            continue;
        }
        let [p, q, r] = [a, b, c].map(|v| mesh.vertices[v as usize]);
        if is_flat(p, q, r) {
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
    let (near_duplicate_pairs, coincident_pairs) = close_pairs(mesh, &undirected_edges);

    MeshReport {
        is_closed,
        is_manifold,
        is_consistently_oriented: consistently_oriented,
        is_tag_length_valid: mesh.tags.is_empty() || mesh.tags.len() == mesh.triangles.len(),
        boundary_edges,
        non_manifold_edges,
        degenerate_triangles,
        total_edges: undirected_edges.len(),
        near_duplicate_pairs,
        coincident_pairs,
    }
}

/// The vertex pairs closer than the near-duplicate radius: (apart, at one position and joined by no
/// edge; an edge between two at one position makes flat triangles, which are counted already).
/// Each used vertex goes in a grid of cells one radius wide; a pair within the radius lies in one cell
/// or two neighbouring ones, so each cell is compared with itself and its 13 forward neighbours.
fn close_pairs(mesh: &TriMesh, edges: &HashMap<(u32, u32), (u32, u32)>) -> (usize, usize) {
    let mut used: Vec<u32> = mesh.triangles.iter().flatten().copied().collect();
    used.sort_unstable();
    used.dedup();
    let scale = used.iter().fold(1.0f64, |s, &v| {
        let p = mesh.vertices[v as usize];
        s.max(p.x.abs()).max(p.y.abs()).max(p.z.abs())
    });
    let radius = NEAR_DUPLICATE * scale;
    let cell = |p: Vec3d| [(p.x / radius).floor() as i64, (p.y / radius).floor() as i64, (p.z / radius).floor() as i64];
    let mut sorted: Vec<([i64; 3], u32)> = used.iter().map(|&v| (cell(mesh.vertices[v as usize]), v)).collect();
    sorted.sort_unstable();
    let mut cells: FastMap<[i64; 3], (usize, usize)> = FastMap::default();
    let mut start = 0;
    while start < sorted.len() {
        let mut end = start + 1;
        while end < sorted.len() && sorted[end].0 == sorted[start].0 {
            end += 1;
        }
        cells.insert(sorted[start].0, (start, end));
        start = end;
    }
    let (mut apart, mut together) = (0, 0);
    let mut compare = |i: u32, j: u32| {
        let (p, q) = (mesh.vertices[i as usize], mesh.vertices[j as usize]);
        if (q - p).length_sq() >= radius * radius {
            return;
        }
        if !(p.x == q.x && p.y == q.y && p.z == q.z) {
            apart += 1;
        } else if !edges.contains_key(&(i.min(j), i.max(j))) {
            together += 1;
        }
    };
    for (&c, &(s, e)) in &cells {
        for a in s..e {
            for b in a + 1..e {
                compare(sorted[a].1, sorted[b].1);
            }
        }
        // The 13 neighbours after this cell (the other 13 see it as theirs).
        for offset in FORWARD {
            if let Some(&(s2, e2)) = cells.get(&[c[0] + offset[0], c[1] + offset[1], c[2] + offset[2]]) {
                for a in s..e {
                    for b in s2..e2 {
                        compare(sorted[a].1, sorted[b].1);
                    }
                }
            }
        }
    }
    (apart, together)
}

/// Half of a cell's 26 neighbours: those after it in (x, y, z) order.
const FORWARD: [[i64; 3]; 13] = [
    [0, 0, 1],
    [0, 1, -1], [0, 1, 0], [0, 1, 1],
    [1, -1, -1], [1, -1, 0], [1, -1, 1],
    [1, 0, -1], [1, 0, 0], [1, 0, 1],
    [1, 1, -1], [1, 1, 0], [1, 1, 1],
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_unit_cube;

    #[test]
    fn pieces_join_triangles_only_across_edges() {
        assert_eq!(pieces(&TriMesh::new()), 0, "an empty mesh has no pieces");
        let mut mesh = make_unit_cube();
        assert_eq!(pieces(&mesh), 1, "a cube is one piece");
        let mut other = make_unit_cube();
        for p in &mut other.vertices {
            p.x += 2.0;
        }
        mesh.merge(&other);
        assert_eq!(pieces(&mesh), 2, "two disjoint cubes are two pieces");

        mesh.triangles = vec![[0, 1, 2], [2, 3, 4]];
        assert_eq!(pieces(&mesh), 2, "sharing only a vertex keeps pieces separate");
        mesh.triangles.push([2, 1, 3]);
        assert_eq!(pieces(&mesh), 1, "an edge bridge joins both pieces");
    }

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
    fn close_pairs_count_copies_apart_and_contacts() {
        let report = validate_mesh(&make_unit_cube());
        assert_eq!((report.near_duplicate_pairs, report.coincident_pairs), (0, 0), "a cube has no close pairs");
        let shifted = |d: f64| {
            let mut copy = make_unit_cube();
            for p in &mut copy.vertices {
                *p += d;
            }
            let mut mesh = make_unit_cube();
            mesh.merge(&copy);
            validate_mesh(&mesh)
        };
        let report = shifted(1e-10);
        assert_eq!((report.near_duplicate_pairs, report.coincident_pairs), (8, 0),
            "each corner and its copy 1e-10 away are a near-duplicate pair");
        let report = shifted(1.0);
        assert_eq!((report.near_duplicate_pairs, report.coincident_pairs), (0, 1),
            "cubes touching at one corner keep one coincident pair");
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
