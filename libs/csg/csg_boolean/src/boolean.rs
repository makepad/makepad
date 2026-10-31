use makepad_csg_math::portable::PortableFloat;
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

use crate::classify::{classify_triangles_in, MeshAccel, TriLocation};
use crate::corefine::corefine_in;
use crate::thread_pool;
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;

/// Boolean operation type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

/// Finishing parameters for `mesh_boolean_with`.
///
/// The defaults are tuned for the model scales of the example/golden suite
/// (units of ~1..100). Callers that author in metres with legitimate
/// sub-millimetre detail (e.g. LocalGen: a 1e-4 altitude cutoff deleted a
/// closing face from 35 mm dog legs after the fourth union) should pass finer
/// values through `mesh_boolean_with` — topology validation downstream stays
/// the final authority.
#[derive(Clone, Copy, Debug)]
pub struct FinishParams {
    /// Weld / T-junction snapping tolerance for the intersection region.
    pub tolerance: f64,
}

impl Default for FinishParams {
    fn default() -> Self {
        FinishParams { tolerance: 1e-4 }
    }
}

/// Perform a boolean operation on two triangle meshes.
pub fn mesh_boolean(mesh_a: &TriMesh, mesh_b: &TriMesh, op: BoolOp) -> TriMesh {
    mesh_boolean_with(mesh_a, mesh_b, op, FinishParams::default())
}

/// Perform a boolean operation with explicit finishing parameters.
pub fn mesh_boolean_with(
    mesh_a: &TriMesh,
    mesh_b: &TriMesh,
    op: BoolOp,
    finish: FinishParams,
) -> TriMesh {
    let (prep_a, prep_b) = (MeshAccel::build(mesh_a), MeshAccel::build(mesh_b));
    mesh_boolean_in(mesh_a, &prep_a, mesh_b, &prep_b, op, finish)
}

/// [`mesh_boolean_with`] of prepared meshes: each operand's triangles and
/// tree serve the overlap search, the classification of the other operand's
/// triangles (against the operand as given: the same surface as its
/// corefined copy) and the coplanar checks, so a mesh used in several
/// booleans is prepared once.
pub fn mesh_boolean_in(
    mesh_a: &TriMesh,
    prep_a: &MeshAccel,
    mesh_b: &TriMesh,
    prep_b: &MeshAccel,
    op: BoolOp,
    finish: FinishParams,
) -> TriMesh {
    // Step 1: Corefine both meshes
    let coref = corefine_in(mesh_a, prep_a, mesh_b, prep_b);
    if thread_pool::cancelled() {
        return TriMesh::new();
    }

    // Step 2: Classify triangles
    let class_a = classify_triangles_in(&coref.mesh_a, prep_b, &coref.on_boundary_a);
    let class_b = classify_triangles_in(&coref.mesh_b, prep_a, &coref.on_boundary_b);
    if thread_pool::cancelled()
        || class_a.len() != coref.mesh_a.triangle_count()
        || class_b.len() != coref.mesh_b.triangle_count()
    {
        return TriMesh::new();
    }

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

    // Did corefinement actually cut anything? With zero intersection segments
    // both meshes were copied through untouched, no boundary faces exist, and
    // the whole finishing tail (weld + T-junction repair + sliver removal)
    // has nothing to repair.
    let untouched = coref.num_segments == 0;

    // Point-in-mesh queries for the coplanar checks during selection go to
    // the prepared operands; they are only consulted for boundary faces.
    let (accel_b, accel_a): (Option<&MeshAccel>, Option<&MeshAccel>) = if untouched { (None, None) } else { (Some(prep_b), Some(prep_a)) };

    let mut result = TriMesh::new();
    // Per-result-vertex flag: is this vertex referenced by any boundary
    // (intersection-region) triangle? The finishing passes start from these.
    let mut active: Vec<bool> = Vec::new();
    // Lazy old-index -> result-index remaps keep the result mesh indexed
    // instead of exploding it to 3 vertices per triangle. This is what makes
    // the `untouched` early return below sound: a zero-segment result is a
    // verbatim indexed subset of its inputs, not an unindexed triangle soup.
    let mut remap_a: Vec<u32> = vec![u32::MAX; coref.mesh_a.vertex_count()];
    let mut remap_b: Vec<u32> = vec![u32::MAX; coref.mesh_b.vertex_count()];

    fn emit_tri(
        result: &mut TriMesh,
        active: &mut Vec<bool>,
        src: &TriMesh,
        remap: &mut [u32],
        tri: [u32; 3],
        boundary: bool,
        reverse: bool,
    ) {
        let mut out = [0u32; 3];
        for (k, &vi) in tri.iter().enumerate() {
            let m = &mut remap[vi as usize];
            if *m == u32::MAX {
                *m = result.add_vertex(src.vertices[vi as usize]);
                active.push(false);
            }
            if boundary {
                active[*m as usize] = true;
            }
            out[k] = *m;
        }
        if reverse {
            result.add_triangle(out[0], out[2], out[1]);
        } else {
            result.add_triangle(out[0], out[1], out[2]);
        }
    }

    // From mesh A: A has priority for surface-coplanar faces
    for ti in 0..coref.mesh_a.triangle_count() {
        if thread_pool::cancelled() {
            return TriMesh::new();
        }
        let c = class_a[ti];
        // A face of A touching B face to face: inside a union (dropped), no
        // volume of an intersection (dropped), still A's surface after A - B.
        let mut keep = match op {
            BoolOp::Union => c == TriLocation::Outside,
            BoolOp::Difference => c == TriLocation::Outside || c == TriLocation::Touching,
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
            // accel_b exists whenever boundary faces exist (untouched == false).
            let inside = accel_b
                .as_ref()
                .map(|acc| acc.point_inside(outside_point))
                .unwrap_or(false);
            if !inside {
                // Surface-coplanar: this face is on B's surface.
                // Keep it from A (B's duplicate will be dropped).
                keep = true;
            }
        }

        if keep {
            emit_tri(
                &mut result,
                &mut active,
                &coref.mesh_a,
                &mut remap_a,
                coref.mesh_a.triangles[ti],
                coref.on_boundary_a[ti],
                false,
            );
        }
    }

    // From mesh B: keep qualifying faces, drop surface-coplanar duplicates
    for ti in 0..coref.mesh_b.triangle_count() {
        if thread_pool::cancelled() {
            return TriMesh::new();
        }
        let c = class_b[ti];

        // Base selection (a face of B touching A face to face is kept by no
        // operation: inside a union, not inside A for the others).
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
            // accel_a exists whenever boundary faces exist (untouched == false).
            let inside = accel_a
                .as_ref()
                .map(|acc| acc.point_inside(outside_point))
                .unwrap_or(false);
            if !inside {
                // Surface-coplanar: A already has this face.
                continue;
            }
        }

        // For union: also drop OnBoundary faces (they're interior to the union)
        if op == BoolOp::Union && c == TriLocation::OnBoundary {
            continue;
        }

        emit_tri(
            &mut result,
            &mut active,
            &coref.mesh_b,
            &mut remap_b,
            coref.mesh_b.triangles[ti],
            coref.on_boundary_b[ti],
            op == BoolOp::Difference, // reversed winding for difference
        );
    }

    // Weld near-coincident vertices from independent intersection computations.
    // Only vertices of the intersection region can be near-coincident, but the
    // weld is a cheap single hash pass on the (now indexed) result, and running
    // it over the whole mesh keeps behavior identical to the historical
    // pipeline for touching-but-not-intersecting geometry.
    let weld_remap = result.weld_vertices_remap(finish.tolerance);

    // Translate boundary-activity flags across the weld, and mark every vertex
    // that survived a merge (its position now stands in for a moved neighbor).
    let mut welded_active = vec![false; result.vertex_count()];
    let mut group_size = vec![0u32; result.vertex_count()];
    for (old, &new) in weld_remap.iter().enumerate() {
        group_size[new as usize] += 1;
        if active[old] {
            welded_active[new as usize] = true;
        }
    }
    for (i, &g) in group_size.iter().enumerate() {
        if g > 1 {
            welded_active[i] = true;
        }
    }
    let mut active = welded_active;

    // Slivers (a triangle thinner than the tolerance: a corner within it of
    // its longest edge) go by flipping that edge with the triangle across, the
    // triangles the weld collapsed with them: the mesh stays as closed as it was.
    remove_slivers(&mut result, finish.tolerance);

    // A boundary vertex from an earlier boolean becomes an ordinary source
    // vertex in the next boolean, so restricting repair to the *latest*
    // boundary would miss cascading seams from staged unions (the reviewed
    // dog's third and fourth leg). An inherited seam is exactly a run of
    // unpaired (boundary) edges in the welded result: activate their
    // endpoints too, which keeps the repair local to what is actually broken
    // instead of re-scanning the whole mesh.
    mark_open_edge_endpoints(&result, &mut active);

    // Close the seams: split every edge at the active vertices lying on it,
    // all of them at once (one pass, not a pass per inserted vertex).
    fix_mesh_t_junctions(&mut result, finish.tolerance, &active);

    // Final cleanup: the triangles the repair collapsed.
    remove_collapsed_triangles(&mut result);
    remove_duplicate_triangles(&mut result);

    result
}

/// Mark the endpoints of every unpaired (boundary) edge as active.
/// After welding, unpaired edges are exactly the broken places: this op's
/// unstitched T-junction gaps plus any seam inherited from an earlier
/// boolean's marginal output.
fn mark_open_edge_endpoints(mesh: &TriMesh, active: &mut [bool]) {
    let mut keys: Vec<u64> = Vec::with_capacity(mesh.triangles.len() * 3);
    for &[a, b, c] in &mesh.triangles {
        keys.extend([edge_key(a, b), edge_key(b, c), edge_key(c, a)]);
    }
    keys.sort_unstable();
    let mut i = 0;
    while i < keys.len() {
        let mut j = i + 1;
        while j < keys.len() && keys[j] == keys[i] {
            j += 1;
        }
        if j - i == 1 {
            active[(keys[i] >> 32) as usize] = true;
            active[keys[i] as u32 as usize] = true;
        }
        i = j;
    }
}

/// Triangles on one vertex triple after the weld. Coplanar corefinement can select the same
/// tiny face through both source meshes (same winding): one stays, two would make its edges
/// non-manifold. A fold (the weld pressing two faces of opposite winding onto one triple)
/// is no surface: the pair goes, so the edges around it stay paired. In general the net
/// winding of a triple stays: none, or one triangle of the winding in excess.
fn remove_duplicate_triangles(mesh: &mut TriMesh) {
    use makepad_csg_math::fast_hash::FastMap;
    // Per sorted triple: (net winding, the first triangle of each winding).
    let mut net: FastMap<[u32; 3], (i32, Option<usize>, Option<usize>)> = FastMap::default();
    let winding = |t: [u32; 3]| -> bool {
        // Even rotation of the sorted triple: + ; odd: -.
        let mut s = t;
        s.sort_unstable();
        let r = t.iter().position(|v| *v == s[0]).unwrap();
        t[(r + 1) % 3] == s[1]
    };
    for (ti, t) in mesh.triangles.iter().enumerate() {
        let mut key = *t;
        key.sort_unstable();
        let e = net.entry(key).or_insert((0, None, None));
        if winding(*t) {
            e.0 += 1;
            e.1.get_or_insert(ti);
        } else {
            e.0 -= 1;
            e.2.get_or_insert(ti);
        }
    }
    let mut keep = vec![false; mesh.triangles.len()];
    for (_, (n, plus, minus)) in net {
        match n {
            0 => {}
            n if n > 0 => keep[plus.unwrap()] = true,
            _ => keep[minus.unwrap()] = true,
        }
    }
    let mut i = 0;
    mesh.triangles.retain(|_| {
        i += 1;
        keep[i - 1]
    });
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
// After corefinement + face selection + weld, the result may have
// T-junctions: a vertex from one side's CDT retriangulation lies on an edge
// of a triangle from the other side, or a near-degenerate triangle has its
// apex on its long edge, whose neighbour across that edge must be split too
// or removing the sliver later opens a hole.
//
// Fix: per edge of the region (each undirected edge once), find every active
// vertex lying on it with a grid walk along the edge, and split every
// triangle on that edge at all of them at once (except a triangle's own
// apex): a fan from the opposite corner, nested when several edges of one
// triangle are split. That is the triangulation repeated single splits
// converge to, in one pass over the region instead of one pass per inserted
// vertex.

/// Fix T-junctions in a welded mesh by splitting triangles at the active
/// vertices that lie on their edges.
fn fix_mesh_t_junctions(mesh: &mut TriMesh, tol: f64, active: &[bool]) {
    // A split inserts no vertex and every active vertex on a split edge
    // becomes a corner, so a later round can only find one on a new fan
    // diagonal (degenerate input): it looks at the new triangles only.
    let mut from = 0;
    for _ in 0..4 {
        if thread_pool::cancelled() {
            return;
        }
        match fix_t_junctions_pass(mesh, tol, active, from) {
            Some(first_new) => from = first_new,
            None => return,
        }
    }
}

fn edge_key(u: u32, v: u32) -> u64 {
    let (a, b) = if u < v { (u, v) } else { (v, u) };
    ((a as u64) << 32) | b as u64
}

/// One pass over the triangles from index `from` on: returns, if any was
/// split, the index of the first new triangle (they go at the end).
fn fix_t_junctions_pass(mesh: &mut TriMesh, tol: f64, active: &[bool], from: usize) -> Option<usize> {
    let num_tris = mesh.triangles.len();
    let seam: Vec<u32> = (0..mesh.vertices.len() as u32).filter(|&v| active.get(v as usize).copied().unwrap_or(true)).collect();
    if num_tris <= from || seam.is_empty() {
        return None;
    }

    // Grid over the active vertices, about one per cell, sized from their
    // own bounds (not the mesh's: far-apart operands would crowd it).
    let mut lo = mesh.vertices[seam[0] as usize];
    let mut hi = lo;
    for &v in &seam {
        let p = mesh.vertices[v as usize];
        lo = dvec3(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = dvec3(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let cells_per_axis = (seam.len() as f64).pcbrt().max(4.0);
    let cell = ((hi - lo).length() / cells_per_axis).max(tol * 2.0);
    let inv_cell = 1.0 / cell;
    let cell_of = |p: Vec3d| -> (i64, i64, i64) {
        ((p.x * inv_cell).floor() as i64, (p.y * inv_cell).floor() as i64, (p.z * inv_cell).floor() as i64)
    };
    let mut grid: makepad_csg_math::fast_hash::FastMap<(i64, i64, i64), Vec<u32>> = Default::default();
    for &v in &seam {
        grid.entry(cell_of(mesh.vertices[v as usize])).or_default().push(v);
    }
    let (reg_min, reg_max) = (lo - dvec3(tol, tol, tol), hi + dvec3(tol, tol, tol));

    // The half-edges (triangle * 3 + slot, slot k = corner k -> corner k+1)
    // of triangles reaching the active region, grouped by undirected edge.
    let mut half: Vec<(u64, u32)> = Vec::new();
    for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate().skip(from) {
        let (va, vb, vc) = (mesh.vertices[a as usize], mesh.vertices[b as usize], mesh.vertices[c as usize]);
        if va.x.max(vb.x).max(vc.x) < reg_min.x
            || va.x.min(vb.x).min(vc.x) > reg_max.x
            || va.y.max(vb.y).max(vc.y) < reg_min.y
            || va.y.min(vb.y).min(vc.y) > reg_max.y
            || va.z.max(vb.z).max(vc.z) < reg_min.z
            || va.z.min(vb.z).min(vc.z) > reg_max.z
        {
            continue;
        }
        // Only an edge between two active vertices can carry one: a vertex
        // lies on an edge of a triangle that meets the cut (all of whose
        // corners are active), of an inherited seam (open: its ends are
        // active) or of a sliver next to them.
        let t = ti as u32 * 3;
        for (k, (u, v)) in [(a, b), (b, c), (c, a)].into_iter().enumerate() {
            if active.get(u as usize).copied().unwrap_or(true) && active.get(v as usize).copied().unwrap_or(true) {
                half.push((edge_key(u, v), t + k as u32));
            }
        }
    }
    half.sort_unstable();

    // Per half-edge, the active vertices strictly inside it, ordered from
    // its start.
    let mut splits: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut near: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < half.len() {
        if thread_pool::cancelled() {
            return None;
        }
        let mut j = i + 1;
        while j < half.len() && half[j].0 == half[i].0 {
            j += 1;
        }
        let (u, v) = ((half[i].0 >> 32) as u32, half[i].0 as u32);
        let (pu, pv) = (mesh.vertices[u as usize], mesh.vertices[v as usize]);
        // Walk the edge in steps of at most one cell: a point within `tol` of
        // the edge is within `cell / 2 + tol` (per axis) of a step point.
        // A short edge (the usual case near a cut) takes the cells of its
        // grown bounds directly.
        near.clear();
        let scan = |a: Vec3d, b: Vec3d, near: &mut Vec<u32>| {
            let (x0, y0, z0) = cell_of(a);
            let (x1, y1, z1) = cell_of(b);
            for gx in x0..=x1 {
                for gy in y0..=y1 {
                    for gz in z0..=z1 {
                        if let Some(bucket) = grid.get(&(gx, gy, gz)) {
                            near.extend_from_slice(bucket);
                        }
                    }
                }
            }
        };
        let (bmin, bmax) = (pu.min(pv) - dvec3(tol, tol, tol), pu.max(pv) + dvec3(tol, tol, tol));
        let span = (bmax - bmin) * inv_cell;
        let steps = ((pv - pu).length() * inv_cell).ceil().max(1.0) as usize;
        if (span.x + 2.0) * (span.y + 2.0) * (span.z + 2.0) <= 27.0 * (steps + 1) as f64 {
            scan(bmin, bmax, &mut near);
        } else {
            let reach = dvec3(cell * 0.5 + tol, cell * 0.5 + tol, cell * 0.5 + tol);
            for s in 0..=steps {
                let p = pu + (pv - pu) * (s as f64 / steps as f64);
                scan(p - reach, p + reach, &mut near);
            }
        }
        near.sort_unstable();
        near.dedup();
        let uv = pv - pu;
        let mut on: Vec<(f64, u32)> = near
            .iter()
            .filter(|&&w| w != u && w != v && point_on_edge(mesh.vertices[w as usize], pu, pv, tol))
            .map(|&w| ((mesh.vertices[w as usize] - pu).dot(uv), w))
            .collect();
        if !on.is_empty() {
            on.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
            for &(_, h) in &half[i..j] {
                let t = mesh.triangles[(h / 3) as usize];
                let k = (h % 3) as usize;
                // Ordered from this half-edge's start. Every half-edge of the edge splits at
                // every vertex on it, its own triangle's apex included (that triangle is a
                // sliver: its pieces collapse and go), so the edge's two sides stay paired.
                let forward = t[k] == u;
                let mut list: Vec<u32> = on.iter().map(|x| x.1).collect();
                if !forward {
                    list.reverse();
                }
                if !list.is_empty() {
                    splits.push((h, list));
                }
            }
        }
        i = j;
    }
    if splits.is_empty() {
        return None;
    }
    splits.sort_unstable_by_key(|s| s.0);

    // Retriangulate each split triangle; a triangle's split edges are adjacent.
    let mut removed = vec![false; num_tris];
    let mut new_triangles: Vec<[u32; 3]> = Vec::new();
    let mut i = 0;
    while i < splits.len() {
        let ti = (splits[i].0 / 3) as usize;
        let mut on_edge: [&[u32]; 3] = [&[], &[], &[]];
        while i < splits.len() && (splits[i].0 / 3) as usize == ti {
            on_edge[(splits[i].0 % 3) as usize] = &splits[i].1;
            i += 1;
        }
        removed[ti] = true;
        split_triangle(mesh.triangles[ti], on_edge, &mut new_triangles);
    }
    let old_tris = std::mem::take(&mut mesh.triangles);
    mesh.triangles = old_tris.into_iter().zip(removed).filter(|(_, r)| !r).map(|(t, _)| t).collect();
    let first_new = mesh.triangles.len();
    mesh.triangles.extend(new_triangles);
    Some(first_new)
}

/// Split triangle `[a, b, c]` at the points on its edges (`on[0]` on a->b,
/// `on[1]` on b->c, `on[2]` on c->a, each ordered from the edge's start):
/// the first split edge fans from the opposite corner, and the fan's end
/// triangles, which carry the other two edges, are split in turn.
fn split_triangle(t: [u32; 3], on: [&[u32]; 3], out: &mut Vec<[u32; 3]>) {
    let Some(k) = (0..3).find(|&k| !on[k].is_empty()) else {
        out.push(t);
        return;
    };
    // Rotate so the split edge is a->b; windings are kept.
    let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
    let (pts, on_bc, on_ca) = (on[k], on[(k + 1) % 3], on[(k + 2) % 3]);
    let n = pts.len();
    for j in 0..=n {
        let u = if j == 0 { a } else { pts[j - 1] };
        let v = if j == n { b } else { pts[j] };
        // The first fan triangle keeps edge c->a, the last keeps b->c.
        let ca: &[u32] = if j == 0 { on_ca } else { &[] };
        let bc: &[u32] = if j == n { on_bc } else { &[] };
        split_triangle([u, v, c], [&[], bc, ca], out);
    }
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

/// Slivers: a corner within `tol` of its triangle's longest edge (cascaded booleans leave
/// them at altitudes ~1e-6; real thin faces of overlap regions stand ~0.01 off). Each goes
/// by flipping that longest edge with the triangle across it: the sliver's apex then splits
/// the neighbour in two, closing in the same step the T-junction a deleted sliver would
/// leave, so a closed mesh stays closed. A sliver with no single triangle across (an open
/// or non-manifold edge) stays for the T-junction repair. Triangles with a repeated corner
/// (an edge the weld collapsed) go first.
fn remove_slivers(mesh: &mut TriMesh, tol: f64) {
    use makepad_csg_math::fast_hash::FastMap;
    let thin = |m: &TriMesh, t: [u32; 3]| -> Option<usize> {
        let p = t.map(|i| m.vertices[i as usize]);
        let len = [(p[1] - p[0]).length(), (p[2] - p[1]).length(), (p[0] - p[2]).length()];
        let k = (0..3).max_by(|a, b| len[*a].total_cmp(&len[*b])).unwrap();
        let area2 = (p[1] - p[0]).cross(p[2] - p[0]).length();
        // Edge k runs from corner k to corner k + 1; its apex is corner k + 2.
        (len[k] > 0.0 && area2 / len[k] < tol).then_some(k)
    };
    mesh.triangles.retain(|&[a, b, c]| a != b && b != c && c != a);
    for _round in 0..8 {
        let mut half: FastMap<(u32, u32), Vec<usize>> = FastMap::default();
        for (ti, t) in mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                half.entry((t[k], t[(k + 1) % 3])).or_default().push(ti);
            }
        }
        let mut done = vec![false; mesh.triangles.len()];
        let mut drop = vec![false; mesh.triangles.len()];
        let mut changed = false;
        for ti in 0..mesh.triangles.len() {
            if done[ti] {
                continue;
            }
            let t = mesh.triangles[ti];
            let Some(k) = thin(mesh, t) else { continue };
            let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            let Some(across) = half.get(&(b, a)).filter(|n| n.len() == 1).map(|n| n[0]) else { continue };
            if across == ti || done[across] {
                continue;
            }
            let n = mesh.triangles[across];
            let Some(j) = (0..3).find(|&j| n[j] == b && n[(j + 1) % 3] == a) else { continue };
            let d = n[(j + 2) % 3];
            done[ti] = true;
            done[across] = true;
            changed = true;
            if d == c {
                // The two fold onto each other: both go.
                drop[ti] = true;
                drop[across] = true;
            } else {
                mesh.triangles[ti] = [a, d, c];
                mesh.triangles[across] = [d, b, c];
            }
        }
        if !changed {
            break;
        }
        let mut i = 0;
        mesh.triangles.retain(|_| {
            i += 1;
            !drop[i - 1]
        });
    }
}

/// Drop the triangles with a repeated corner (an edge the weld collapsed).
fn remove_collapsed_triangles(mesh: &mut TriMesh) {
    mesh.triangles.retain(|&[a, b, c]| a != b && b != c && c != a);
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

