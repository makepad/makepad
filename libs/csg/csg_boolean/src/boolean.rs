use makepad_csg_math::portable::PortableFloat;
// Corefinement-based boolean operations.
//
// - Preserves original triangles away from the intersection
// - Produces high-quality mesh output
// - Scales as O(n log n + k) where k = intersection edges
//
// Algorithm:
// 1. Corefine both meshes (split along intersection curve)
// 2. Order both surfaces round every edge where they meet, into cells
// 3. Keep the patches whose two cells the operation tells apart

use crate::cells::{select, Surfaces};
use crate::classify::{InsideTest, MeshAccel};
use crate::corefine::corefine_with;
use crate::thread_pool;
use makepad_csg_math::fast_hash::FastMap;
use makepad_csg_math::{dvec3, BBox3d, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::borrow::Cow;

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
    /// Upper bound on weld / T-junction snapping, and the sliver altitude cutoff.
    /// Snapping is also bounded by the numerical plane-coincidence tolerance.
    pub tolerance: f64,
}

impl Default for FinishParams {
    fn default() -> Self {
        FinishParams { tolerance: 1e-4 }
    }
}

/// What one boolean decided on the way, for tests and sweeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BooleanReport {
    pub classes: usize,
    pub aliases: usize,
    pub refused: usize,
    /// Cut post-conditions broken: sites with more than two ends; kept boundaries that are not
    /// the chain cycle; segments that are not an edge of the pieces.
    pub cut: [usize; 3],
    /// Open edges, by vertex index, of each operand's refined mesh (not counting its seam); this,
    /// `joined_open`, `joined_nonmanifold` and `finished_nonmanifold` are counted only by
    /// [`mesh_boolean_report`].
    pub refined_open: [usize; 2],
    /// Open edges after selection, joined by vertex index only (before any weld), not counting the
    /// seam edges a kept passed-through operand shares.
    pub joined_open: usize,
    /// Exact predicate evaluations in the cut's triangulations.
    pub exact: usize,
    pub ties: usize,
    pub degenerate: usize,
    /// Label groups with evidence both ways: an operand that is not a solid in the virtual set.
    pub label_conflicts: usize,
    /// Edges where the surfaces meet whose signs make no one order round the edge.
    pub degenerate_walls: usize,
    /// Edges where the surfaces meet at which an operand has faces, but not one each way.
    pub unpaired_walls: usize,
    /// Edges where the surfaces meet that only one refined surface has (a broken cut, or the two
    /// cuts subdividing a segment differently).
    pub lone_walls: usize,
    /// Exact evaluations of the signs round those edges.
    pub wall_exact: usize,
    /// Parts with no edge where the surfaces meet and no seam, labelled by a ray test.
    pub ray_tests: usize,
    /// Edges with more than one face either way after selection, joined by vertex index only
    /// (before any weld). With `joined_open` 0 too, the join is closed and manifold by index.
    pub joined_nonmanifold: usize,
    /// Edges with more than one face either way in the finished mesh.
    pub finished_nonmanifold: usize,
    /// Weld joins refused because an edge they fuse would end with more than one face either way.
    pub weld_refused: usize,
}

/// Perform a boolean operation on two triangle meshes.
pub fn mesh_boolean(mesh_a: &TriMesh, mesh_b: &TriMesh, op: BoolOp) -> TriMesh {
    mesh_boolean_with(mesh_a, mesh_b, op, FinishParams::default())
}

/// Perform a boolean operation with explicit finishing parameters.
///
/// Only what the other operand can reach takes part: each operand's triangles outside the other's box, grown by
/// every distance the boolean treats as coincident, lie outside the other solid and pass through as they are,
/// on the vertices they had. The rest is corefined, classified and finished, its seam with the passed-through
/// triangles kept: a cutter at one end of a large mesh costs what it cuts.
pub fn mesh_boolean_with(
    mesh_a: &TriMesh,
    mesh_b: &TriMesh,
    op: BoolOp,
    finish: FinishParams,
) -> TriMesh {
    mesh_boolean_work(mesh_a, mesh_b, op, finish).0
}

/// [`mesh_boolean_with`], and its work: the triangles that took part, both operands' (the rest passed through).
/// A diagnostic, the regression guard for a boolean costing what it cuts, counted where a clock would vary.
pub fn mesh_boolean_work(
    mesh_a: &TriMesh,
    mesh_b: &TriMesh,
    op: BoolOp,
    finish: FinishParams,
) -> (TriMesh, usize) {
    let (box_a, box_b) = (mesh_a.bounding_box(), mesh_b.bounding_box());
    let all = box_a.union(box_b);
    // The broad phase admits pairs within the plane tolerance, the weld and the T-junction repair snap within
    // the finishing tolerance: past both, nothing of one operand can touch the other.
    let reach = finish.tolerance.max(crate::tri_tri::plane_tolerance(all.min, all.max, all.min, all.max, all.min, all.max));
    let tagged = mesh_a.is_tagged() || mesh_b.is_tagged();
    let (a, b) = (Operand::within(mesh_a, box_b.grow(reach), tagged), Operand::within(mesh_b, box_a.grow(reach), tagged));
    let took = a.near.triangle_count() + b.near.triangle_count();
    (boolean_of(&a, &b, op, finish, false).0, took)
}

/// [`mesh_boolean_with`], and what it decided.
pub fn mesh_boolean_report(
    mesh_a: &TriMesh,
    mesh_b: &TriMesh,
    op: BoolOp,
    finish: FinishParams,
) -> (TriMesh, BooleanReport) {
    let (box_a, box_b) = (mesh_a.bounding_box(), mesh_b.bounding_box());
    let all = box_a.union(box_b);
    let reach = finish.tolerance.max(crate::tri_tri::plane_tolerance(all.min, all.max, all.min, all.max, all.min, all.max));
    let tagged = mesh_a.is_tagged() || mesh_b.is_tagged();
    let (a, b) = (Operand::within(mesh_a, box_b.grow(reach), tagged), Operand::within(mesh_b, box_a.grow(reach), tagged));
    boolean_of(&a, &b, op, finish, true)
}

/// [`mesh_boolean_with`] of prepared meshes: each operand's triangles and
/// tree serve the overlap search, the classification of the other operand's
/// triangles (against the operand as given: the same surface as its
/// corefined copy) and the coplanar checks, so a mesh used in several
/// booleans is prepared once. Every triangle takes part.
pub fn mesh_boolean_in(
    mesh_a: &TriMesh,
    prep_a: &MeshAccel,
    mesh_b: &TriMesh,
    prep_b: &MeshAccel,
    op: BoolOp,
    finish: FinishParams,
) -> TriMesh {
    let tagged = mesh_a.is_tagged() || mesh_b.is_tagged();
    boolean_of(&Operand::whole(mesh_a, prep_a, tagged), &Operand::whole(mesh_b, prep_b, tagged), op, finish, false).0
}

/// An operand as a boolean sees it: the triangles the other operand can reach, and the rest.
struct Operand<'a> {
    mesh: &'a TriMesh,
    /// The triangles that take part, on all of `mesh`'s vertices (the same indices and bounds); `mesh` itself
    /// when every triangle does.
    near: Cow<'a, TriMesh>,
    near_accel: Cow<'a, MeshAccel>,
    /// The others: outside the other operand's box, so outside its solid. They pass through as they are.
    far: Vec<[u32; 3]>,
    near_tags: Vec<u32>,
    far_tags: Vec<u32>,
    /// The solid `mesh` bounds, for the other operand's points.
    inside: InsideTest,
}

impl<'a> Operand<'a> {
    fn whole(mesh: &'a TriMesh, accel: &'a MeshAccel, tagged: bool) -> Self {
        let near_tags = if !tagged { Vec::new() } else if mesh.is_tagged() { mesh.tags.clone() } else { vec![0; mesh.triangle_count()] };
        Operand { mesh, near: Cow::Borrowed(mesh), near_accel: Cow::Borrowed(accel), far: Vec::new(), near_tags, far_tags: Vec::new(), inside: InsideTest::whole(accel) }
    }

    /// The triangles of `mesh` that meet `reach` (the other operand's grown box) take part. The other operand's
    /// points all lie in its own box, inside `reach`, where `mesh`'s surface is those triangles: they are tested
    /// against them alone (with three reference points of `reach`).
    fn within(mesh: &'a TriMesh, reach: BBox3d, tagged: bool) -> Self {
        let mut near = Vec::new();
        let mut far = Vec::new();
        let mut near_tags = Vec::new();
        let mut far_tags = Vec::new();
        for (ti, &t) in mesh.triangles.iter().enumerate() {
            let (a, b, c) = mesh.triangle_vertices(ti);
            if BBox3d::from_triangle(a, b, c).intersects(reach) {
                near.push(t);
                if tagged { near_tags.push(mesh.tag(ti)); }
            } else {
                far.push(t);
                if tagged { far_tags.push(mesh.tag(ti)); }
            }
        }
        if far.is_empty() {
            let accel = MeshAccel::build(mesh);
            let inside = InsideTest::whole(&accel);
            return Operand { mesh, near: Cow::Borrowed(mesh), near_accel: Cow::Owned(accel), far, near_tags, far_tags, inside };
        }
        let mut part = TriMesh::with_capacity(mesh.vertex_count(), near.len());
        part.vertices.extend_from_slice(&mesh.vertices);
        part.triangles = near;
        let accel = MeshAccel::build(&part);
        let inside = InsideTest::local(&accel, reach, mesh);
        Operand { mesh, near: Cow::Owned(part), near_accel: Cow::Owned(accel), far, near_tags, far_tags, inside }
    }

    /// The vertices the passed-through triangles share with the ones that take part, in index order.
    fn seam(&self) -> Vec<u32> {
        if self.far.is_empty() {
            return Vec::new();
        }
        let mut far = vec![false; self.mesh.vertex_count()];
        for t in &self.far {
            for &v in t {
                far[v as usize] = true;
            }
        }
        let mut seam = vec![false; self.mesh.vertex_count()];
        for t in &self.near.triangles {
            for &v in t {
                seam[v as usize] = far[v as usize];
            }
        }
        (0..seam.len() as u32).filter(|&v| seam[v as usize]).collect()
    }
}

/// The boolean of the operands' parts that take part, with their passed-through triangles joined back on.
///
/// The seam between the two keeps every vertex and edge: the passed-through triangles lie outside the other
/// operand's grown box, where the corefinement splits nothing. The seam vertices go into the result first and the
/// weld leaves them as they are (they keep their positions for the passed-through triangles, and coincident copies
/// there, an earlier boolean's contact fans, stay apart); the T-junction repair splits no seam edge, and the
/// contact separation keeps their indices (their fans are partly passed through).
/// What changes against finishing the whole mesh: coincident vertices, slivers, duplicates, T-junctions and open
/// edges among the passed-through triangles stay as they were (the finishing of the boolean that made them saw
/// them already).
/// `count_open` counts the report's open edges (two passes over every edge only tests and sweeps read).
fn boolean_of(a: &Operand, b: &Operand, op: BoolOp, finish: FinishParams, count_open: bool) -> (TriMesh, BooleanReport) {
    let tagged = a.mesh.is_tagged() || b.mesh.is_tagged();
    let (mesh_a, mesh_b) = (a.mesh, b.mesh);
    // Step 1: Corefine both meshes
    let coref = corefine_with(&a.near, &a.near_accel, &b.near, &b.near_accel, count_open);
    if thread_pool::cancelled() {
        return (TriMesh::new(), BooleanReport::default());
    }

    // Step 2: cells. A vertex an operand shares with its passed-through triangles lies outside the
    // other operand: the patches there have its outside on both sides.
    let point_id = |operand: usize, v: u32| if operand == 0 { v } else { coref.b_ids[v as usize] };
    let seam_vertices = [a.seam(), b.seam()];
    let seams = [0, 1].map(|operand| {
        if seam_vertices[operand].is_empty() {
            return Vec::new();
        }
        let mut seam = vec![false; coref.mesh_a.vertex_count()];
        for &v in &seam_vertices[operand] {
            seam[point_id(operand, v) as usize] = true;
        }
        seam
    });
    let selection = select(&Surfaces {
        meshes: [&coref.mesh_a, &coref.mesh_b],
        origins: [&coref.origin_a, &coref.origin_b],
        walls: &coref.walls,
        exact: &coref.exact,
        seams: [&seams[0], &seams[1]],
        inside: [&a.inside, &b.inside],
    }, op);
    if thread_pool::cancelled() {
        return (TriMesh::new(), BooleanReport::default());
    }

    // Step 3: emit the kept patches, A's then B's.
    let mut result = TriMesh::new();
    // Per-result-vertex flag: is this vertex referenced by any boundary
    // (intersection-region) triangle? The finishing passes start from these.
    let mut active: Vec<bool> = Vec::new();
    // Both refined meshes use point ids, so selection joins their shared points by index.
    let mut remap: Vec<u32> = vec![u32::MAX; coref.mesh_a.vertex_count()];

    // The passed-through triangles a result keeps: those of A (outside B) in a union and a difference, those of
    // B (outside A) in a union. Their seam vertices come first (`locked`: the result's first `locked` vertices).
    let keep_far = [op != BoolOp::Intersection, op == BoolOp::Union];
    for index in 0..2 {
        if keep_far[index] {
            for &v in &seam_vertices[index] {
                let id = point_id(index, v) as usize;
                if remap[id] == u32::MAX {
                    remap[id] = result.add_vertex(coref.mesh_a.vertices[id]);
                    active.push(false);
                }
            }
        }
    }
    let locked = result.vertex_count();
    // The seam's edges, which the passed-through triangles share: never split here.
    let mut seam_edges: Vec<u64> = Vec::new();
    for (index, operand) in [a, b].into_iter().enumerate() {
        if keep_far[index] {
            for t in &operand.far {
                for k in 0..3 {
                    let (u, v) = (remap[point_id(index, t[k]) as usize], remap[point_id(index, t[(k + 1) % 3]) as usize]);
                    if u != u32::MAX && v != u32::MAX {
                        seam_edges.push(edge_key(u, v));
                    }
                }
            }
        }
    }
    seam_edges.sort_unstable();
    seam_edges.dedup();

    fn emit_tri(
        result: &mut TriMesh,
        active: &mut Vec<bool>,
        src: &TriMesh,
        remap: &mut [u32],
        tri: [u32; 3],
        boundary: bool,
        reverse: bool,
        tag: Option<u32>,
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
            result.triangles.push([out[0], out[2], out[1]]);
        } else {
            result.triangles.push([out[0], out[1], out[2]]);
        }
        if let Some(tag) = tag { result.tags.push(tag); }
    }

    let tags = [&a.near_tags, &b.near_tags];
    let mut kept_a = 0;
    for (operand, (mesh, origin)) in [(&coref.mesh_a, &coref.origin_a), (&coref.mesh_b, &coref.origin_b)].into_iter().enumerate() {
        let cut = if operand == 0 { &coref.on_boundary_a } else { &coref.on_boundary_b };
        for ti in 0..mesh.triangle_count() {
            if thread_pool::cancelled() {
                return (TriMesh::new(), BooleanReport::default());
            }
            if selection.keep[operand][ti] {
                emit_tri(
                    &mut result,
                    &mut active,
                    mesh,
                    &mut remap,
                    mesh.triangles[ti],
                    cut[ti],
                    selection.reverse[operand][ti],
                    tagged.then(|| tags[operand][origin[ti] as usize]),
                );
            }
        }
        if operand == 0 {
            kept_a = result.triangles.len();
        }
    }

    // A point both operands' kept triangles share (a contact, or a class vertex B aliased onto A)
    // is where the weld used to merge their copies: it is active as a merged vertex is.
    let mut from = vec![0u8; result.vertex_count()];
    for (ti, t) in result.triangles.iter().enumerate() {
        for &v in t {
            from[v as usize] |= if ti < kept_a { 1 } else { 2 };
        }
    }
    for (v, &operands) in from.iter().enumerate() {
        if operands == 3 {
            active[v] = true;
        }
    }

    let counts = if count_open { directed_counts(&result) } else { FastMap::default() };
    let mut report = BooleanReport {
        classes: coref.report.classes,
        aliases: coref.report.aliases,
        refused: coref.report.refused,
        cut: coref.report.cut,
        refined_open: coref.report.refined_open,
        joined_open: counts.iter().filter(|(edge, &(ab, ba))| ab + ba == 1 && seam_edges.binary_search(edge).is_err()).count(),
        joined_nonmanifold: counts.values().filter(|&&(ab, ba)| ab > 1 || ba > 1).count(),
        exact: coref.report.exact,
        ties: coref.report.ties,
        degenerate: coref.report.degenerate,
        label_conflicts: selection.report.label_conflicts,
        degenerate_walls: selection.report.degenerate_walls,
        unpaired_walls: selection.report.unpaired_walls,
        lone_walls: selection.report.lone_walls,
        wall_exact: selection.report.wall_exact,
        ray_tests: selection.report.ray_tests,
        ..BooleanReport::default()
    };

    // Join numerically coincident constructions, not distinct short edges.
    // The finishing tolerance can exceed genuine cap detail: merging those
    // vertices moves the surface, and independent copies can even choose
    // different representatives as the weld's spatial grid fills, opening a
    // previously matching seam. Use the same coincidence scale as the plane
    // predicates, with the caller's tolerance as an upper bound.
    let bounds = mesh_a.bounding_box().union(mesh_b.bounding_box());
    let coincidence = crate::tri_tri::plane_tolerance(
        bounds.min, bounds.max, bounds.min, bounds.max, bounds.min, bounds.max,
    ).min(finish.tolerance);
    // The seam vertices are fixed: they keep their positions for the passed-through triangles, and coincident
    // copies there (an earlier boolean's contact fans) stay apart as those triangles have them.
    // A join that would leave an edge with more than one face either way is refused: a thin cell of
    // exact geometry stays as it is.
    let (weld_remap, weld_refused) = crate::weld::weld_keeping_edges(&mut result, locked, coincidence);
    report.weld_refused = weld_refused;

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
    remove_slivers(&mut result, finish.tolerance, &mut active);

    // A boundary vertex from an earlier boolean becomes an ordinary source
    // vertex in the next boolean, so restricting repair to the *latest*
    // boundary would miss cascading seams from staged unions (the reviewed
    // dog's third and fourth leg). An inherited seam is exactly a run of
    // unpaired (boundary) edges in the welded result: activate their
    // endpoints too, which keeps the repair local to what is actually broken
    // instead of re-scanning the whole mesh.
    mark_open_edge_endpoints(&result, &mut active);

    // Close the seams: split every edge at the active vertices lying on it,
    // all of them at once (one pass, not a pass per inserted vertex). As with
    // welding, a merely nearby vertex belongs to real thin geometry: inserting
    // it would bend the edge and change the surface instead of subdividing it.
    fix_mesh_t_junctions(&mut result, coincidence, &active, &seam_edges);

    // Final cleanup: the triangles the repair collapsed.
    remove_collapsed_triangles(&mut result);
    remove_duplicate_triangles(&mut result);

    // A seam vertex's fan is partly in the passed-through triangles: it keeps its index.
    active[..locked].fill(false);
    crate::contact::separate_contact_fans(&mut result, &active);

    // The passed-through triangles, on the seam vertices (as welded) and their own.
    let near = result.triangles.len();
    for (index, operand) in [a, b].into_iter().enumerate() {
        if !keep_far[index] || operand.far.is_empty() {
            continue;
        }
        let mut own = vec![u32::MAX; operand.mesh.vertex_count()];
        for (ti, t) in operand.far.iter().enumerate() {
            let t = t.map(|v| match remap[point_id(index, v) as usize] {
                u32::MAX => {
                    let o = &mut own[v as usize];
                    if *o == u32::MAX {
                        *o = result.add_vertex(operand.mesh.vertices[v as usize]);
                    }
                    *o
                }
                seam => seam,
            });
            result.triangles.push(t);
            if tagged { result.tags.push(operand.far_tags[ti]); }
        }
    }
    remove_flat_triangles(&mut result, near);
    if count_open {
        report.finished_nonmanifold = directed_counts(&result).values().filter(|&&(ab, ba)| ab > 1 || ba > 1).count();
    }
    (result, report)
}

/// The flat triangles among the first `near` (validation's degenerate rule, `is_flat`) that the
/// finishing left: a T-junction split beside two near-coincident constructions, a corner the weld
/// set on the line through the other two. The corner opposite a flat triangle's longest edge lies
/// on that edge, so flipping the edge with the triangle across splits that triangle at the
/// corner: the surface stays and the flat triangle goes. A flip never makes an edge that is
/// already there, so the mesh stays as closed as it was; two triangles folded onto one triple both
/// go.
fn remove_flat_triangles(mesh: &mut TriMesh, near: usize) {
    use makepad_csg_math::fast_hash::FastMap;
    use makepad_csg_mesh::validate::is_flat;
    let flat = |m: &TriMesh, t: [u32; 3]| {
        t[0] != t[1] && t[1] != t[2] && t[2] != t[0] && is_flat(m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize])
    };
    let mut todo: Vec<usize> = (0..near).filter(|&ti| flat(mesh, mesh.triangles[ti])).collect();
    if todo.is_empty() {
        return;
    }
    mesh.clear_normals();
    // Each directed edge's triangle; usize::MAX where two have it (a non-manifold edge: no flip there).
    let mut half: FastMap<(u32, u32), usize> = FastMap::default();
    for (ti, t) in mesh.triangles.iter().enumerate() {
        for k in 0..3 {
            half.entry((t[k], t[(k + 1) % 3])).and_modify(|e| *e = usize::MAX).or_insert(ti);
        }
    }
    let mut dropped = vec![false; mesh.triangles.len()];
    // A flip can leave another flat triangle (a corner across that is on the line too): a few
    // rounds, as for slivers.
    for _round in 0..8 {
        let mut next = Vec::new();
        for ti in todo {
            let t = mesh.triangles[ti];
            if dropped[ti] || !flat(mesh, t) {
                continue;
            }
            let p = t.map(|v| mesh.vertices[v as usize]);
            let len = [(p[1] - p[0]).length_sq(), (p[2] - p[1]).length_sq(), (p[0] - p[2]).length_sq()];
            let k = (0..3).max_by(|x, y| len[*x].total_cmp(&len[*y])).unwrap();
            let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            let Some(&ni) = half.get(&(b, a)) else { continue };
            if ni == usize::MAX || ni == ti || dropped[ni] {
                continue;
            }
            let n = mesh.triangles[ni];
            let d = n.into_iter().find(|v| *v != a && *v != b).unwrap();
            if d != c && (half.contains_key(&(c, d)) || half.contains_key(&(d, c))) {
                continue;
            }
            for e in [(a, b), (b, c), (c, a), (b, a), (a, d), (d, b)] {
                half.remove(&e);
            }
            if d == c {
                // Folded onto one triple: both go.
                dropped[ti] = true;
                dropped[ni] = true;
                continue;
            }
            mesh.triangles[ti] = [a, d, c];
            mesh.triangles[ni] = [d, b, c];
            for (e, owner) in [((a, d), ti), ((d, c), ti), ((c, a), ti), ((d, b), ni), ((b, c), ni), ((c, d), ni)] {
                half.insert(e, owner);
            }
            if mesh.is_tagged() {
                mesh.tags[ti] = mesh.tags[ni];
            }
            next.extend([ti, ni]);
        }
        if next.is_empty() {
            break;
        }
        todo = next;
    }
    if dropped.iter().any(|d| *d) {
        mesh.retain_triangles(|ti, _| !dropped[ti]);
    }
}

/// Per undirected edge (`edge_key`), its faces running from the lower index to the higher and back.
fn directed_counts(mesh: &TriMesh) -> FastMap<u64, (usize, usize)> {
    let mut counts: FastMap<u64, (usize, usize)> = FastMap::default();
    for &[a, b, c] in &mesh.triangles {
        for (a, b) in [(a, b), (b, c), (c, a)] {
            let count = counts.entry(edge_key(a, b)).or_default();
            if a < b { count.0 += 1 } else { count.1 += 1 }
        }
    }
    counts
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
    mesh.retain_triangles(|i, _| keep[i]);
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
fn fix_mesh_t_junctions(mesh: &mut TriMesh, tol: f64, active: &[bool], fixed_edges: &[u64]) {
    // A split inserts no vertex and every active vertex on a split edge
    // becomes a corner, so a later round can only find one on a new fan
    // diagonal (degenerate input): it looks at the new triangles only.
    let mut from = 0;
    for _ in 0..4 {
        if thread_pool::cancelled() {
            return;
        }
        match fix_t_junctions_pass(mesh, tol, active, fixed_edges, from) {
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
/// `fixed_edges` (sorted [`edge_key`]s) are never split: the other triangles on them are not in `mesh`.
fn fix_t_junctions_pass(mesh: &mut TriMesh, tol: f64, active: &[bool], fixed_edges: &[u64], from: usize) -> Option<usize> {
    let num_tris = mesh.triangles.len();
    let tagged = mesh.is_tagged();
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
    for (ti, &[a, b, c]) in mesh.triangles.iter().enumerate() {
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
                let key = edge_key(u, v);
                if fixed_edges.binary_search(&key).is_err() {
                    half.push((key, t + k as u32));
                }
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
        // Two opposite half-edges already close this edge. A nearby vertex
        // on another edge is not a T-junction: splitting here can flatten a
        // thin wall into zero-area faces or overlapping folds.
        if j - i == 2 {
            let h0 = half[i].1;
            let h1 = half[i + 1].1;
            let t0 = mesh.triangles[(h0 / 3) as usize];
            let t1 = mesh.triangles[(h1 / 3) as usize];
            if t0[(h0 % 3) as usize] != t1[(h1 % 3) as usize] {
                i = j;
                continue;
            }
        }
        // Count both sides even when one triangle predates this round.
        if half[i..j].iter().all(|&(_, h)| (h / 3) < from as u32) {
            i = j;
            continue;
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
    let mut new_tags = Vec::new();
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
        if tagged { new_tags.resize(new_triangles.len(), mesh.tags[ti]); }
    }
    mesh.retain_triangles(|ti, _| !removed[ti]);
    let first_new = mesh.triangles.len();
    mesh.triangles.extend(new_triangles);
    if tagged { mesh.tags.extend(new_tags); }
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
pub(crate) fn remove_slivers(mesh: &mut TriMesh, tol: f64, active: &mut [bool]) {
    use makepad_csg_math::fast_hash::{FastMap, FastSet};
    let tagged = mesh.is_tagged();
    mesh.clear_normals();
    let altitude = |m: &TriMesh, t: [u32; 3]| -> (usize, f64) {
        let p = t.map(|i| m.vertices[i as usize]);
        let len = [(p[1] - p[0]).length(), (p[2] - p[1]).length(), (p[0] - p[2]).length()];
        let k = (0..3).max_by(|a, b| len[*a].total_cmp(&len[*b])).unwrap();
        let area2 = (p[1] - p[0]).cross(p[2] - p[0]).length();
        // Edge k runs from corner k to corner k + 1; its apex is corner k + 2.
        (k, if len[k] > 0.0 { area2 / len[k] } else { 0.0 })
    };
    mesh.retain_triangles(|_, &[a, b, c]| a != b && b != c && c != a);
    // Vertices never move here. Only a flipped triangle needs its altitude
    // recomputed; the sentinel invalidates both replacements for the next round.
    let mut heights: Vec<(usize, f64)> = mesh.triangles.iter().map(|&t| altitude(mesh, t)).collect();
    // The candidates' apexes, and every edge at one of them: a flip never makes an edge that is
    // already there (in a thin cell the other sheet often has it), as in `remove_flat_triangles`.
    let mut apex: Vec<bool> = Vec::new();
    for _round in 0..8 {
        let mut candidates = Vec::new();
        let mut half: FastMap<(u32, u32), (usize, usize)> = FastMap::default();
        for (ti, t) in mesh.triangles.iter().enumerate() {
            if heights[ti].0 == usize::MAX {
                heights[ti] = altitude(mesh, *t);
            }
            let (k, height) = heights[ti];
            // Keep the original comparison: NaN heights/tolerances also proceed.
            if height >= tol { continue; }
            candidates.push(ti);
            half.entry((t[(k + 1) % 3], t[k])).or_default();
        }
        if candidates.is_empty() {
            break;
        }
        apex.resize(mesh.vertices.len(), false);
        for &ti in &candidates {
            apex[mesh.triangles[ti][(heights[ti].0 + 2) % 3] as usize] = true;
        }
        let mut at_apex: FastSet<(u32, u32)> = FastSet::default();
        // The original round-start map is consulted only for these reverse
        // edges. Count all their occurrences, keeping the sole triangle index.
        for (ti, t) in mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                let (u, v) = (t[k], t[(k + 1) % 3]);
                if apex[u as usize] || apex[v as usize] {
                    at_apex.insert((u.min(v), u.max(v)));
                }
                if let Some((count, across)) = half.get_mut(&(u, v)) {
                    *count += 1;
                    *across = ti;
                }
            }
        }
        for &ti in &candidates {
            apex[mesh.triangles[ti][(heights[ti].0 + 2) % 3] as usize] = false;
        }
        let mut done = vec![false; mesh.triangles.len()];
        let mut drop = vec![false; mesh.triangles.len()];
        let mut changed = false;
        for ti in candidates {
            if done[ti] {
                continue;
            }
            let t = mesh.triangles[ti];
            let (k, height) = heights[ti];
            if height >= tol { continue; }
            let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            let Some(across) = half.get(&(b, a)).filter(|n| n.0 == 1).map(|n| n.1) else { continue };
            if across == ti || done[across] {
                continue;
            }
            let n = mesh.triangles[across];
            let Some(j) = (0..3).find(|&j| n[j] == b && n[(j + 1) % 3] == a) else { continue };
            let d = n[(j + 2) % 3];
            if d != c && at_apex.contains(&(c.min(d), c.max(d))) {
                continue;
            }
            if d != c {
                // A diagonal flip preserves the surface only in a plane.
                // A thin exposed cap may meet a side wall at its longest
                // edge; flipping across that crease changes solid volume.
                // Use the larger face's normal so a nearly collinear sliver
                // cannot make the plane test ill-conditioned.
                let p = t.map(|v| mesh.vertices[v as usize]);
                let q = n.map(|v| mesh.vertices[v as usize]);
                let normal_t = (p[1] - p[0]).cross(p[2] - p[0]);
                let normal_n = (q[1] - q[0]).cross(q[2] - q[0]);
                let plane_tol = crate::tri_tri::plane_tolerance(p[0], p[1], p[2], q[0], q[1], q[2]);
                let coplanar = if normal_t.length_sq() >= normal_n.length_sq() {
                    (mesh.vertices[d as usize] - p[0]).dot(normal_t).abs() <= plane_tol * normal_t.length()
                } else {
                    (mesh.vertices[c as usize] - q[0]).dot(normal_n).abs() <= plane_tol * normal_n.length()
                };
                if !coplanar {
                    continue;
                }
                // A flip must improve the thin face and preserve winding.
                // Flipping a non-convex or collinear quad creates a zero-area
                // closing face, which disappears when a consumer emits it.
                let replacements = [[a, d, c], [d, b, c]];
                if replacements.iter().any(|&r| altitude(mesh, r).1 <= height) {
                    continue;
                }
                let preserves = |old: [u32; 3], new: [u32; 3]| {
                    let p = old.map(|v| mesh.vertices[v as usize]);
                    let normal = (p[1] - p[0]).cross(p[2] - p[0]);
                    let (i,j) = if normal.x.abs() >= normal.y.abs() && normal.x.abs() >= normal.z.abs() {
                        (1,2)
                    } else if normal.y.abs() >= normal.z.abs() { (0,2) } else { (0,1) };
                    let orient = |t: [u32; 3]| {
                        let p = t.map(|v| { let p = mesh.vertices[v as usize]; [p.x,p.y,p.z] });
                        makepad_csg_math::orient2d(p[0][i],p[0][j],p[1][i],p[1][j],p[2][i],p[2][j])
                    };
                    let (before, after) = (orient(old), orient(new));
                    after != 0.0 && (before > 0.0) == (after > 0.0)
                };
                if !preserves(t, replacements[0]) || !preserves(n, replacements[1]) { continue; }
            }
            for v in [a, b, c, d] { active[v as usize] = true; }
            done[ti] = true;
            done[across] = true;
            changed = true;
            if d == c {
                // The two fold onto each other: both go.
                drop[ti] = true;
                drop[across] = true;
            } else {
                if tagged && mesh.tags[ti] != mesh.tags[across] {
                    let tags = [mesh.tags[ti], mesh.tags[across]];
                    let first = flipped_tag(mesh, [a, d, c], [t, n], tags);
                    let second = flipped_tag(mesh, [d, b, c], [t, n], tags);
                    mesh.tags[ti] = first;
                    mesh.tags[across] = second;
                }
                mesh.triangles[ti] = [a, d, c];
                mesh.triangles[across] = [d, b, c];
                at_apex.remove(&(a.min(b), a.max(b)));
                at_apex.insert((c.min(d), c.max(d)));
                heights[ti].0 = usize::MAX;
                heights[across].0 = usize::MAX;
            }
        }
        if !changed {
            break;
        }
        let mut i = 0;
        heights.retain(|_| {
            i += 1;
            !drop[i - 1]
        });
        mesh.retain_triangles(|i, _| !drop[i]);
    }
}

/// Choose the old triangle containing a replacement's projected centroid, or the nearest one.
fn flipped_tag(mesh: &TriMesh, new: [u32; 3], old: [[u32; 3]; 2], tags: [u32; 2]) -> u32 {
    let p = new.map(|v| mesh.vertices[v as usize]);
    let center = p[0] + ((p[1] - p[0]) + (p[2] - p[0])) / 3.0;
    let mut distances = [f64::INFINITY; 2];
    for i in 0..2 {
        let [a, b, c] = old[i].map(|v| mesh.vertices[v as usize]);
        let normal = (b - a).cross(c - a);
        let norm_sq = normal.length_sq();
        if norm_sq > 0.0 {
            let projected = center - normal * ((center - a).dot(normal) / norm_sq);
            // Cross products avoid subtracting nearly equal squared edge lengths on slivers.
            let bary = [
                (b - projected).cross(c - projected).dot(normal) / norm_sq,
                (c - projected).cross(a - projected).dot(normal) / norm_sq,
                (a - projected).cross(b - projected).dot(normal) / norm_sq,
            ];
            if bary.iter().all(|&v| v >= -64.0 * f64::EPSILON) {
                return tags[i];
            }
        }
        // Outside the projected triangle (or a collapsed one), the closest point is on an edge.
        for (a, b) in [(a, b), (b, c), (c, a)] {
            let edge = b - a;
            let t = if edge.length_sq() > 0.0 {
                ((center - a).dot(edge) / edge.length_sq()).clamp(0.0, 1.0)
            } else { 0.0 };
            distances[i] = distances[i].min(center.distance_sq(a + edge * t));
        }
    }
    tags[if distances[0] <= distances[1] { 0 } else { 1 }]
}

/// Drop the triangles with a repeated corner (an edge the weld collapsed).
fn remove_collapsed_triangles(mesh: &mut TriMesh) {
    mesh.clear_normals();
    mesh.retain_triangles(|_, &[a, b, c]| a != b && b != c && c != a);
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
}

