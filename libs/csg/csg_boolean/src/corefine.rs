// Corefinement by point identity.
//
// Broad phase finds pairs within the possible motion of both triangles.
// Coplanar classes place input vertices; exact sites name intersection keys.
// A key pool and edge table share points and edge runs across every cut.
// Cut triangles keep those ids, and both refined meshes use one vertex array.

use crate::cells::Exact;
use crate::classes::{build_classes, REACH};
use crate::classify::MeshAccel;
use crate::cut::{construct, cut_triangle, CutTriangle, EdgeTable, KeyPool};
use crate::keys::Key;
use crate::sites::{pair_sites, PairTriangle, PlaneRecords, WindingMemo};
use crate::tri_tri::{coplanar_tri_tri, plane_tolerance, Planes};
use makepad_csg_math::fast_hash::{FastMap, FastSet};
use makepad_csg_math::{orient2d, thread_pool, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::sync::Arc;

/// What the corefinement decided, for tests and sweeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CorefineReport {
    pub classes: usize,
    pub aliases: usize,
    pub refused: usize,
    /// Broken sites, broken cut boundaries, and missing segment edges.
    pub cut: [usize; 3],
    /// Open refined edges, excluding edges already open in the input by point id; counted only for a
    /// report ([`crate::boolean::mesh_boolean_report`]).
    pub refined_open: [usize; 2],
    /// Exact predicate evaluations in the cut's triangulations.
    pub exact: usize,
    pub ties: usize,
    pub degenerate: usize,
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
    /// The refined edges classification does not flood across, as ascending id pairs: the pieces'
    /// edges along the intersection segments, and every edge of a cut that broke a post-condition
    /// (a segment missing from its pieces, a boundary that is not its chain, a zero normal), so
    /// each of its pieces is classified alone.
    pub walls: FastSet<[u32; 2]>,
    /// The exact geometry the classification reads: the virtual points, the constructed points
    /// and the input triangles on point ids.
    pub(crate) exact: Exact,
    /// Number of intersection segments before deduplication.
    pub num_segments: usize,
    /// For each input vertex of B, its point id (both refined meshes index one array of point ids).
    pub b_ids: Vec<u32>,
    /// What the corefinement decided, for tests and sweeps.
    pub report: CorefineReport,
}

/// Perform mesh corefinement on two triangle meshes.
///
/// Both meshes are split along their intersection curve so that the intersection
/// is represented as edges in both output meshes.
pub fn corefine(mesh_a: &TriMesh, mesh_b: &TriMesh) -> CorefinementResult {
    corefine_in(mesh_a, &MeshAccel::build(mesh_a), mesh_b, &MeshAccel::build(mesh_b))
}

/// [`corefine`] of prepared meshes (their triangle lists and trees built once).
pub fn corefine_in(mesh_a: &TriMesh, prep_a: &MeshAccel, mesh_b: &TriMesh, prep_b: &MeshAccel) -> CorefinementResult {
    corefine_with(mesh_a, prep_a, mesh_b, prep_b, false)
}

/// [`corefine_in`]; `count_open` also counts the refined meshes' open edges in the report, two
/// passes over every edge of both operands that only tests and sweeps read.
pub(crate) fn corefine_with(
    mesh_a: &TriMesh,
    prep_a: &MeshAccel,
    mesh_b: &TriMesh,
    prep_b: &MeshAccel,
    count_open: bool,
) -> CorefinementResult {
    const CHUNK: usize = 128;
    let bounds = mesh_a.bounding_box().union(mesh_b.bounding_box());
    let tolerance = plane_tolerance(
        bounds.min, bounds.max, bounds.min, bounds.max, bounds.min, bounds.max,
    );
    // Both triangles' corners can move up to REACH * tolerance onto their classes.
    let pairs = prep_a.tree().find_overlaps_with_tolerance(prep_b.tree(), (2.0 * REACH + 1.0) * tolerance);
    // The plane records of the triangles the pairs name, geometric half: the scan reads it, and `pair_sites`
    // the full record after the classes.
    let geometric = Arc::new(PlaneRecords::geometric(&pairs, [prep_a.triangles(), prep_b.triangles()]));
    let coplanar = {
        let (prep_a, prep_b, geometric) = (prep_a.clone(), prep_b.clone(), geometric.clone());
        parallel_chunks(pairs.clone(), CHUNK, move |chunk| {
            let (tris_a, tris_b) = (prep_a.triangles(), prep_b.triangles());
            let mut coplanar = Vec::new();
            for &(ai, bi) in chunk {
                if thread_pool::cancelled() {
                    break;
                }
                let (ta, tb) = (tris_a[ai as usize], tris_b[bi as usize]);
                // `coplanar` needs all three corners of one triangle within the band of the other's plane.
                let (plane_a, plane_b) = (geometric.get(0, ai), geometric.get(1, bi));
                if [ta.0, ta.1, ta.2].iter().any(|&x| plane_b.beyond(x, tolerance))
                    && [tb.0, tb.1, tb.2].iter().any(|&x| plane_a.beyond(x, tolerance))
                {
                    continue;
                }
                let Some(planes) = Planes::unless_apart(ta, tb, tolerance) else { continue };
                if planes.coplanar(ta, tb) && coplanar_tri_tri(ta.0, ta.1, ta.2, tb.0, tb.1, tb.2) {
                    coplanar.push((ai, bi));
                }
            }
            coplanar
        })
    };
    let classes = Arc::new(build_classes(mesh_a, mesh_b, &coplanar, tolerance));
    let (n_a, n_b) = (mesh_a.vertex_count() as u32, mesh_b.vertex_count() as u32);
    let b_ids: Vec<u32> = (n_a..n_a + n_b).map(|id| classes.alias.get(&id).copied().unwrap_or(id)).collect();
    let triangles: Arc<[Vec<[u32; 3]>; 2]> = Arc::new([
        mesh_a.triangles.clone(),
        mesh_b.triangles.iter().map(|t| t.map(|v| b_ids[v as usize])).collect(),
    ]);
    let sites = {
        let memo = Arc::new(WindingMemo::new([triangles[0].len(), triangles[1].len()]));
        let planes = Arc::new(geometric.displaced(&classes.points, [&mesh_a.triangles, &mesh_b.triangles], n_a));
        let (classes, triangles) = (classes.clone(), triangles.clone());
        parallel_chunks(pairs, CHUNK, move |chunk| {
            let mut sites = Vec::new();
            for &(ai, bi) in chunk {
                if thread_pool::cancelled() {
                    break;
                }
                let found = pair_sites(
                    &classes,
                    &memo,
                    &planes,
                    PairTriangle { ids: triangles[0][ai as usize], index: ai },
                    PairTriangle { ids: triangles[1][bi as usize], index: bi },
                );
                if !found.segments.is_empty() || !found.on_edges.is_empty() || found.broken != 0 || found.coplanar {
                    sites.push((ai, bi, found));
                }
            }
            sites
        })
    };
    let mut report = CorefineReport {
        classes: classes.count,
        aliases: classes.alias.len(),
        refused: classes.refused.iter().sum(),
        ..CorefineReport::default()
    };
    let mut pool = KeyPool::new(n_a + n_b);
    // Per operand, (triangle, segment) for the triangles the sites cut; sorted, so each triangle's segments
    // are one run, ascending and without repeats.
    let mut segments: [Vec<(u32, [u32; 2])>; 2] = [Vec::new(), Vec::new()];
    let mut coplanar = FastSet::default();
    let mut on_edges = Vec::new();
    let mut num_segments = 0;
    for (ai, bi, sites) in sites {
        if thread_pool::cancelled() {
            break;
        }
        report.cut[0] += sites.broken;
        if sites.coplanar {
            coplanar.insert([ai, bi]);
        }
        for segment in sites.segments {
            if thread_pool::cancelled() {
                break;
            }
            let [a, b] = segment.map(|key| pool.id(key));
            if a != b {
                let segment = [a.min(b), a.max(b)];
                segments[0].push((ai, segment));
                segments[1].push((bi, segment));
                num_segments += 1;
            }
        }
        for (_, edge, key) in sites.on_edges {
            debug_assert!(matches!(key, Key::V(_)), "edge sites must name input vertices");
            if let Key::V(v) = key {
                on_edges.push((edge, v));
            }
        }
    }
    for segments in &mut segments {
        if thread_pool::cancelled() {
            break;
        }
        segments.sort_unstable();
        segments.dedup();
    }
    // The exact constructions are independent of each other: make them on the pool.
    let built = {
        let (classes, triangles) = (classes.clone(), triangles.clone());
        parallel_chunks(pool.constructed_keys().to_vec(), 32, move |chunk| {
            chunk.iter()
                .map(|&key| construct(&classes.points, [&triangles[0], &triangles[1]], key))
                .collect()
        })
    };
    pool.set_points(built);
    let table = EdgeTable::build(&pool, &classes.points, &on_edges);
    report.ties = table.ties;
    let positions = Arc::new(pool.positions(&classes.points));
    let pool = Arc::new(pool);
    let mut jobs = Vec::new();
    for (operand, prep) in [prep_a, prep_b].into_iter().enumerate() {
        let (list, mut next) = (&segments[operand], 0);
        for (ti, &corners) in triangles[operand].iter().enumerate() {
            if ti % 1024 == 0 && thread_pool::cancelled() {
                break;
            }
            let first = next;
            while next < list.len() && list[next].0 == ti as u32 {
                next += 1;
            }
            if first == next && !table.touches(corners) {
                continue;
            }
            let runs = table.runs(corners);
            if first == next && runs.iter().all(Vec::is_empty) {
                continue;
            }
            let (a, b, c) = prep.triangles()[ti];
            jobs.push((operand, ti, CutTriangle {
                corners,
                runs,
                segments: list[first..next].iter().map(|&(_, segment)| segment).collect(),
                real: [a, b, c],
            }));
        }
    }
    let made = {
        const CHUNK: usize = 4;
        let positions = positions.clone();
        let pool = pool.clone();
        let classes = classes.clone();
        parallel_chunks(jobs, CHUNK, move |chunk| {
            let mut made = Vec::new();
            for (operand, ti, triangle) in chunk {
                if thread_pool::cancelled() {
                    break;
                }
                made.push((*operand, *ti, cut_triangle(triangle, &positions, &|id| {
                    pool.coordinates(&classes.points, id)
                })));
            }
            made
        })
    };
    let (mut pieces, mut cut_count) = ([0usize; 2], [0usize; 2]);
    for (operand, _, cut) in &made {
        report.cut[1] += usize::from(cut.boundary);
        report.cut[2] += cut.missing;
        report.exact += cut.exact;
        report.degenerate += usize::from(cut.degenerate);
        pieces[*operand] += cut.pieces.len();
        cut_count[*operand] += 1;
    }
    let mut made = made.into_iter().peekable();
    let positions = Arc::try_unwrap(positions).unwrap_or_else(|positions| (*positions).clone());
    let mut refined = [TriMesh::new(), TriMesh::new()];
    refined[0].vertices = positions.clone();
    refined[1].vertices = positions;
    let mut origins = [Vec::new(), Vec::new()];
    let mut boundaries = [Vec::new(), Vec::new()];
    let mut walls = FastSet::default();
    for operand in 0..2 {
        let size = triangles[operand].len() - cut_count[operand] + pieces[operand];
        refined[operand].triangles.reserve(size);
        origins[operand].reserve(size);
        boundaries[operand].reserve(size);
        let source = &triangles[operand];
        // Triangles before `start` are emitted; the uncut ones between two cuts go over in one copy each.
        let mut start = 0;
        let mut cancelled = false;
        while let Some((_, ti, cut)) = made.next_if(|&(o, _, _)| o == operand) {
            if thread_pool::cancelled() {
                cancelled = true;
                break;
            }
            refined[operand].triangles.extend_from_slice(&source[start..ti]);
            origins[operand].extend(start as u32..ti as u32);
            boundaries[operand].resize(boundaries[operand].len() + (ti - start), false);
            walls.extend(cut.walls);
            if cut.boundary || cut.missing > 0 || cut.degenerate {
                walls.extend(cut.pieces.iter().flat_map(|p| [[p[0], p[1]], [p[1], p[2]], [p[2], p[0]]])
                    .map(|[u, v]| [u.min(v), u.max(v)]));
            }
            for piece in cut.pieces {
                refined[operand].triangles.push(piece);
                origins[operand].push(ti as u32);
                boundaries[operand].push(true);
            }
            start = ti + 1;
        }
        if !cancelled {
            refined[operand].triangles.extend_from_slice(&source[start..]);
            origins[operand].extend(start as u32..source.len() as u32);
            boundaries[operand].resize(boundaries[operand].len() + (source.len() - start), false);
        }
        if count_open {
            let input_open = open_edges(&triangles[operand]);
            report.refined_open[operand] = open_edges(&refined[operand].triangles).difference(&input_open).count();
        }
    }
    let [mesh_a, mesh_b] = refined;
    let [origin_a, origin_b] = origins;
    let [on_boundary_a, on_boundary_b] = boundaries;
    CorefinementResult {
        mesh_a,
        mesh_b,
        origin_a,
        origin_b,
        on_boundary_a,
        on_boundary_b,
        walls,
        exact: Exact { classes, pool, triangles, coplanar },
        num_segments,
        b_ids,
        report,
    }
}

fn open_edges(triangles: &[[u32; 3]]) -> FastSet<[u32; 2]> {
    let mut counts: FastMap<[u32; 2], usize> = FastMap::default();
    for &[a, b, c] in triangles {
        if thread_pool::cancelled() {
            break;
        }
        for (a, b) in [(a, b), (b, c), (c, a)] {
            *counts.entry([a.min(b), a.max(b)]).or_default() += 1;
        }
    }
    counts.into_iter().filter_map(|(edge, count)| (count == 1).then_some(edge)).collect()
}

/// `work` over `items` in chunks of `chunk` items, their results concatenated in the items' order.
/// The pool takes the chunks as its threads come free, so a run of expensive items (crossing pairs
/// and cuts cluster) spreads over the threads instead of making one thread's share the wall time.
fn parallel_chunks<T, R, F>(items: Vec<T>, chunk: usize, work: F) -> Vec<R>
where
    T: Send + Sync + 'static,
    R: Send + 'static,
    F: Fn(&[T]) -> Vec<R> + Send + Sync + 'static,
{
    let chunks = items.len().div_ceil(chunk).max(1);
    if chunks == 1 {
        return work(&items);
    }
    let size = items.len().div_ceil(chunks);
    let (items, work): (Arc<[T]>, Arc<F>) = (items.into(), Arc::new(work));
    let tasks: Vec<_> = (0..chunks)
        .map(|c| {
            let (items, work) = (items.clone(), work.clone());
            move || work(&items[(c * size).min(items.len())..((c + 1) * size).min(items.len())])
        })
        .collect();
    thread_pool::parallel_for(tasks).into_iter().flatten().collect()
}

/// Corefinement has already found the coplanar pairs and cut their boundaries.
/// Classify each resulting region in that plane, retaining its source winding.
/// A centroid query is sufficient only AFTER those constraints have been cut.
pub(crate) fn coplanar_regions(
    refined: &TriMesh,
    origins: &[u32],
    pairs: &[Vec<u32>],
    source: impl Fn(u32) -> (Vec3d, Vec3d, Vec3d),
    other: impl Fn(u32) -> (Vec3d, Vec3d, Vec3d),
) -> Vec<Option<bool>> {
    let plane = |(a, b, c): (Vec3d, Vec3d, Vec3d)| {
        let normal = (b - a).cross(c - a);
        (normal, crate::tri_tri::plane_quality(normal.length(), a, b, c))
    };
    let mut source_planes = vec![None; pairs.len()];
    let mut other_planes: FastMap<u32, ((Vec3d, Vec3d, Vec3d), (Vec3d, f64))> = FastMap::default();
    // Only coplanar sources need plane data; all their refined faces reuse it.
    for (ai, candidates) in pairs.iter().enumerate() {
        if candidates.is_empty() { continue; }
        let own = source(ai as u32);
        source_planes[ai] = Some((own, plane(own)));
        for &bi in candidates {
            other_planes.entry(bi).or_insert_with(|| {
                let theirs = other(bi);
                (theirs, plane(theirs))
            });
        }
    }
    origins.iter().enumerate().map(|(ti, &origin)| {
        let candidates = &pairs[origin as usize];
        if candidates.is_empty() { return None; }
        let (own, (normal, quality)) = source_planes[origin as usize].unwrap();
        let (u, v, w) = refined.triangle_vertices(ti);
        let centroid = (u + v + w) / 3.0;
        candidates.iter().find_map(|&bi| {
            // Each pair projects onto its better-conditioned plane, as the pair test did.
            let (theirs, (other_normal, other_quality)) = other_planes[&bi];
            let (i, j) = crate::tri_tri::kept_axes(crate::tri_tri::shared_normal_from_quality(normal, quality, other_normal, other_quality));
            let project = |v: Vec3d| { let v = [v.x, v.y, v.z]; (v[i], v[j]) };
            let winding = orient_2d_tuple(project(own.0), project(own.1), project(own.2));
            let (a, b, c) = (project(theirs.0), project(theirs.1), project(theirs.2));
            let other_winding = orient_2d_tuple(a, b, c);
            (other_winding != 0.0 && point_in_triangle_2d(project(centroid), a, b, c))
                .then_some((winding > 0.0) == (other_winding > 0.0))
        })
    }).collect()
}

fn orient_2d_tuple(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    orient2d(a.0, a.1, b.0, b.1, c.0, c.1)
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
