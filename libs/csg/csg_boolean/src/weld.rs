//! The finishing weld: near-coincident points join only where the join keeps the mesh one face
//! each way on every edge.

use makepad_csg_math::fast_hash::FastMap;
use makepad_csg_mesh::mesh::TriMesh;

/// Joins each vertex from `fixed` on to the first earlier vertex within `tolerance` that the grid
/// finds, as `TriMesh::weld_vertices_remap_after` does, and removes the triangles a join
/// collapses. A join is applied only when every edge it fuses ends with at most two faces, one each
/// way, not counting the triangles it collapses. That one count is the link condition for the two
/// ends of an edge and the stitch for two ends of open edges, and it refuses a join that would press
/// two sheets onto one edge. All candidates of one vertex are tried as one join first, so a cluster
/// of copies of one point joins whole; if that is refused, they join one by one. A refused vertex
/// keeps its own index. Returns the remap (old index to new) and the number of refused joins.
pub(crate) fn weld_keeping_edges(mesh: &mut TriMesh, fixed: usize, tolerance: f64) -> (Vec<u32>, usize) {
    mesh.clear_normals();
    let n = mesh.vertices.len();
    // The candidates, as the plain weld finds them: the first vertex within the radius among the
    // earlier ones that joined nothing.
    let mut target: Vec<u32> = (0..n as u32).collect();
    let tol_sq = tolerance * tolerance;
    let inv_cell = 1.0 / (tolerance * 1.01);
    let mut grid: FastMap<(i64, i64, i64), Vec<u32>> = FastMap::default();
    let mut joins = 0usize;
    for i in fixed..n {
        let p = mesh.vertices[i];
        let cell = ((p.x * inv_cell).floor() as i64, (p.y * inv_cell).floor() as i64, (p.z * inv_cell).floor() as i64);
        let mut found = None;
        'search: for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                for dz in -1i64..=1 {
                    if let Some(bucket) = grid.get(&(cell.0 + dx, cell.1 + dy, cell.2 + dz)) {
                        for &r in bucket {
                            if p.distance_sq(mesh.vertices[r as usize]) < tol_sq {
                                found = Some(r);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        match found {
            Some(r) => {
                target[i] = r;
                joins += 1;
            }
            None => grid.entry(cell).or_default().push(i as u32),
        }
    }

    let mut refused = 0;
    if joins > 0 {
        // Triangles round the vertices that take part in a join, and only those.
        let mut incidence = Incidence { slot: vec![u32::MAX; n], around: Vec::new() };
        let Incidence { slot, around } = &mut incidence;
        for i in fixed..n {
            if target[i] as usize != i {
                for v in [i, target[i] as usize] {
                    if slot[v] == u32::MAX {
                        slot[v] = around.len() as u32;
                        around.push(Vec::new());
                    }
                }
            }
        }
        for (ti, t) in mesh.triangles.iter().enumerate() {
            for &v in t {
                if let Some(list) = around.get_mut(slot[v as usize] as usize) {
                    list.push(ti as u32);
                }
            }
        }
        // Per representative in vertex order, all its candidates at once (a cluster of copies of one
        // point joins whole, where joining them one by one would pass through four-face edges), else
        // one by one in vertex order. `rep` is each vertex's representative so far.
        let mut rep: Vec<u32> = (0..n as u32).collect();
        let mut members: FastMap<u32, Vec<u32>> = FastMap::default();
        let mut joining: FastMap<u32, Vec<u32>> = FastMap::default();
        for i in fixed..n {
            if target[i] as usize != i {
                joining.entry(target[i]).or_default().push(i as u32);
            }
        }
        let mut order: Vec<u32> = joining.keys().copied().collect();
        order.sort_unstable();
        let mut check = Check::default();
        for r in order {
            let cluster = joining.remove(&r).unwrap_or_default();
            if check.keeps_edges(&incidence, &mesh.triangles, &rep, members.get(&r), r, &cluster) {
                for &i in &cluster {
                    rep[i as usize] = r;
                }
                members.entry(r).or_default().extend(cluster);
                continue;
            }
            for i in cluster {
                if check.keeps_edges(&incidence, &mesh.triangles, &rep, members.get(&r), r, &[i]) {
                    rep[i as usize] = r;
                    members.entry(r).or_default().push(i);
                } else {
                    refused += 1;
                }
            }
        }
        target = rep;
    }

    let mut remap = vec![0u32; n];
    let mut vertices = Vec::with_capacity(n - joins + refused);
    for v in 0..n {
        let t = target[v] as usize;
        if t == v {
            remap[v] = vertices.len() as u32;
            vertices.push(mesh.vertices[v]);
        } else {
            remap[v] = remap[t];
        }
    }
    mesh.vertices = vertices;
    for t in &mut mesh.triangles {
        *t = t.map(|v| remap[v as usize]);
    }
    mesh.retain_triangles(|_, &[a, b, c]| a != b && b != c && c != a);
    (remap, refused)
}

/// The triangles round each vertex that takes part in a join: `around[slot[v]]`.
struct Incidence {
    slot: Vec<u32>,
    around: Vec<Vec<u32>>,
}

/// Scratch space for the count rule.
#[derive(Default)]
struct Check {
    triangles: Vec<u32>,
    /// Per other end of an edge at the joined vertex: faces leaving it, faces entering it, the
    /// first vertex of the join that brought one, and whether another one did too.
    ends: FastMap<u32, (u32, u32, u32, bool)>,
}

impl Check {
    /// Whether joining `cluster` (vertices that are their own representatives) into `r` leaves
    /// every edge it fuses with at most one face each way, not counting the triangles it collapses.
    fn keeps_edges(
        &mut self,
        incidence: &Incidence,
        triangles: &[[u32; 3]],
        rep: &[u32],
        members: Option<&Vec<u32>>,
        r: u32,
        cluster: &[u32],
    ) -> bool {
        self.triangles.clear();
        for &v in std::iter::once(&r).chain(members.into_iter().flatten()).chain(cluster) {
            self.triangles.extend_from_slice(&incidence.around[incidence.slot[v as usize] as usize]);
        }
        self.triangles.sort_unstable();
        self.triangles.dedup();
        self.ends.clear();
        for &ti in &self.triangles {
            let before = triangles[ti as usize].map(|v| rep[v as usize]);
            // The one corner the join moves onto `r`: `r` itself or a vertex of the cluster.
            let Some(&from) = before.iter().find(|&&v| v == r || cluster.contains(&v)) else { continue };
            let t = before.map(|v| if cluster.contains(&v) { r } else { v });
            if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
                // Collapsed by this join or an earlier one.
                continue;
            }
            for k in 0..3 {
                let (u, w) = (t[k], t[(k + 1) % 3]);
                let (end, out) = if u == r { (w, true) } else if w == r { (u, false) } else { continue };
                let e = self.ends.entry(end).or_insert((0, 0, from, false));
                if out { e.0 += 1 } else { e.1 += 1 }
                e.3 |= e.2 != from;
            }
        }
        !self.ends.values().any(|&(out, into, _, fused)| fused && (out > 1 || into > 1))
    }
}
