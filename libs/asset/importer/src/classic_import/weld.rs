//! T-junction elimination for classic level geometry.
//!
//! A T-junction is a vertex sitting strictly INSIDE another triangle's edge.
//! The two triangles agree about the surface exactly — there is no overlap
//! and no hole in the maths — but a rasteriser interpolates each edge on its
//! own, so the shared boundary opens a one-pixel hairline that crawls as the
//! camera moves. Doom subsectors, Quake BSP faces and Build sectors all
//! produce them by construction: neighbouring polygons split their shared
//! boundary at different points (texture cell borders, node partitions, wall
//! splits), and nothing in those formats forces the splits to agree.
//!
//! The fix is the classic one: for every triangle edge, insert the vertices
//! that lie on it. Each insertion cuts the triangle from that point to the
//! opposite corner, so no vertex is invented — the inserted position IS the
//! vertex being welded to, bit-for-bit, and UV / colour / normal come from
//! interpolating along the edge, which is exact for a point on it.
//!
//! The pass is orientation-agnostic on purpose: it welds floors to floors,
//! walls to walls, and a wall's foot to the floor edge it stands on, because
//! "is this vertex within half a snap quantum of that edge" is the same
//! question in all three cases.
//!
//! Every classic converter goes through here, including Quake II and Quake
//! III — which for a long time did not, on the reasoning that `qbsp3` and
//! `q3map` already run a T-junction fix so their BSPs arrive welded. They
//! do. But that is about the BSP's OWN faces: these converters then re-cut
//! every face wherever it crosses a texture cell, because the tiles are
//! packed into an atlas and a UV cannot wrap through one, and neighbouring
//! faces do not agree about where those cuts fall. Quake II's demo1 came
//! out of a pre-welded BSP with 6211 fresh cracks, all of the converter's
//! own making. A compiler that welded its input says nothing about a
//! converter that splits its output.
//!
//! Doom, Quake 1 and Build never welded at all — Quake 1's "sparklies" are
//! exactly this defect.

/// Half of `snap_pos`'s 1/512 m quantum: a vertex this close to an edge is
/// ON it — the quantiser could have put it either side.
const ON_EDGE: f32 = 1.0 / 1024.0;

/// A candidate closer than this to either end of an edge IS that end as far
/// as this pass is concerned, and no cut is made.
///
/// The two outcomes are the same size: cutting at distance `d` from a corner
/// leaves a sliver about `d × ON_EDGE` across (the candidate is only
/// guaranteed to be within `ON_EDGE` of the line), and not cutting leaves a
/// crack of the same `d × ON_EDGE`. What differs is that the sliver is
/// geometry which can z-fight its neighbour, so below one source unit —
/// 1/256 m is an eighth of a Doom unit, roughly two of Duke's BUILD units,
/// and twice Doom's own snap quantum — the crack is the better of the two.
/// `t_junctions` in the map tests measures against this same number, because
/// "closer than one source unit to the corner" is not a defect either engine
/// could express. (Every number here doubled with the 2026-08-26 move to the
/// person-pinned metre — same map-unit rule, twice the metres.)
pub(crate) const MIN_FROM_END: f32 = 1.0 / 256.0;

/// Bucket size for the vertex lookup, in metres (64 Doom units), so a cell
/// holds a handful of vertices even in a dense map.
const CELL: f32 = 2.0;

/// Cap on the re-passes described on [`Weld::split`]. Real maps settle in
/// two or three; this only bounds a pathological one.
const MAX_PASSES: usize = 8;

/// The vertex attributes a classic emitter fills. `uvs` is always present;
/// `normals` and `colors` are per-emitter and stay `None` when that emitter
/// does not write them. An attribute vector whose length does not match
/// `positions` is ignored rather than half-rebuilt.
pub(crate) struct Soup<'a> {
    pub positions: &'a mut Vec<[f32; 3]>,
    pub uvs: &'a mut Vec<[f32; 2]>,
    pub normals: Option<&'a mut Vec<[f32; 3]>>,
    pub colors: Option<&'a mut Vec<[f32; 3]>>,
    pub indices: &'a mut Vec<u32>,
}

/// Every vertex a level's parts contribute, ready to be welded into any of
/// them. A level is emitted as several meshes — the static world plus a node
/// per door, lift and hazard floor, plus the sky — and the cracks that show
/// up most are exactly the ones BETWEEN those parts, where a nukage pool or
/// a doorway meets the floor around it. Splitting only ever inserts a
/// position the mesh already had, so the vertex set never changes: one grid
/// built from all parts stays correct however many of them are then welded,
/// and in any order.
pub(crate) struct Weld {
    grid: VertexGrid,
}

impl Weld {
    /// Build from every part's positions — the caller lists them all.
    pub(crate) fn from_parts(parts: &[&[[f32; 3]]]) -> Self {
        Self { grid: VertexGrid::build_many(parts) }
    }

    /// Split one part's edges at the vertices lying on them. Returns how
    /// many splits were inserted; 0 means this part was already clean.
    ///
    /// A cut runs from the point to the opposite corner, and that chord can
    /// itself pass through a vertex that was sitting inside the triangle all
    /// along — a defect no edge split can see. So the pass runs again on its
    /// own output, and keeps going only while the number of T-junctions
    /// LEFT actually falls: two vertices a couple of millimetres apart will
    /// otherwise take turns landing inside a chord cut for the other, and
    /// the mesh grows forever without closing anything. When a pass fails to
    /// improve, its result is thrown away and the previous one stands.
    pub(crate) fn split(&self, soup: Soup<'_>) -> usize {
        let Soup { positions, uvs, normals, colors, indices } = soup;
        let n = positions.len();
        if uvs.len() != n || indices.len() < 3 {
            return 0;
        }
        let mut normals = normals.filter(|v| v.len() == n);
        let mut colors = colors.filter(|v| v.len() == n);
        let mut total = 0usize;
        let mut left = self.residual(positions, indices);
        for _ in 0..MAX_PASSES {
            if left == 0 {
                break;
            }
            let keep = (
                positions.clone(),
                uvs.clone(),
                normals.as_deref().cloned(),
                colors.as_deref().cloned(),
                indices.clone(),
            );
            let split = one_pass(
                &self.grid,
                positions,
                uvs,
                normals.as_deref_mut(),
                colors.as_deref_mut(),
                indices,
            );
            let after = self.residual(positions, indices);
            if split == 0 || after >= left {
                let (kp, ku, kn, kc, ki) = keep;
                *positions = kp;
                *uvs = ku;
                *indices = ki;
                if let (Some(dst), Some(src)) = (normals.as_deref_mut(), kn) {
                    *dst = src;
                }
                if let (Some(dst), Some(src)) = (colors.as_deref_mut(), kc) {
                    *dst = src;
                }
                break;
            }
            total += split;
            left = after;
        }
        total
    }

    /// T-junctions still present in this part, counted against every part's
    /// vertices. Splitting only inserts positions the mesh already had, so
    /// the grid stays valid across passes.
    fn residual(&self, positions: &[[f32; 3]], indices: &[u32]) -> usize {
        let mut hits = 0usize;
        let mut on = Vec::new();
        for tri in indices.chunks_exact(3) {
            let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            if idx.iter().any(|i| *i >= positions.len()) {
                continue;
            }
            let p = [positions[idx[0]], positions[idx[1]], positions[idx[2]]];
            if area2(p[0], p[1], p[2]) <= f32::EPSILON {
                continue;
            }
            for k in 0..3 {
                on.clear();
                self.grid.points_on(p[k], p[(k + 1) % 3], &mut on);
                hits += on.len();
            }
        }
        hits
    }
}

/// Weld a self-contained mesh against its own vertices.
pub(crate) fn split_t_junctions(soup: Soup<'_>) -> usize {
    let weld = Weld::from_parts(&[&soup.positions[..]]);
    weld.split(soup)
}

// ---------------------------------------------------------------------------
// Merging the corners the splitter declines
// ---------------------------------------------------------------------------

/// The defect [`Weld::split`] cannot close, and the only thing that can.
///
/// A cut is refused when it would land inside the chord another cut needs,
/// which happens when two corners sit a hair apart on the same seam: each
/// keeps poisoning the other and the mesh grows without the count falling,
/// so the pass reverts. Duke's E1L1 ends with five of these — pairs of
/// corners one to three BUILD units apart, where a unit is 1/512 m.
///
/// Closing them means MOVING a vertex, which the splitter never does, on
/// purpose: it only ever inserts a position the mesh already had, so it
/// cannot change the surface. This pass is the deliberate exception, and it
/// is why the tolerance is the interesting number rather than an
/// implementation detail. At [`MERGE_TOLERANCE`] the furthest a corner
/// travels is twelve millimetres in a world whose rooms are three storeys of metres tall
/// — invisible, and smaller than the crack it closes, which is the whole
/// justification. E1L1 goes from five residual T-junctions to none; two
/// units leaves two, so three is the number the data asked for and not a
/// round one someone liked.
///
/// Every part merges against ONE table, because the cracks that show up are
/// the ones BETWEEN parts: a doorway's floor against the wall standing on
/// it, a nukage pool against its bank.
pub(crate) struct Merge {
    /// Position bits -> the position it becomes. Only entries that actually
    /// move are stored, so an untouched level costs one empty map.
    moved: std::collections::HashMap<(u32, u32, u32), [f32; 3]>,
}

/// Three source units — the measured number, not a chosen one. One unit is
/// already what [`MIN_FROM_END`] calls indistinguishable; the residual
/// defects are pairs of corners ONE TO THREE units apart, so anything less
/// leaves the widest of them open (two units leaves two of E1L1's five).
pub(crate) const MERGE_TOLERANCE: f32 = 3.0 / 256.0;

impl Merge {
    /// Build the table from every part's positions.
    ///
    /// Deterministic by construction: the unique positions are sorted by
    /// their bit patterns and each is snapped to the first EARLIER canonical
    /// position within tolerance. Rerunning over identical input therefore
    /// produces an identical table, which the byte-identical-rerun rule
    /// needs. Snapping only to earlier canonicals also means a canonical
    /// never moves, so no chain of merges can drag a vertex further than the
    /// tolerance.
    pub(crate) fn from_parts(parts: &[&[[f32; 3]]], tolerance: f32) -> Self {
        let mut seen: std::collections::HashSet<(u32, u32, u32)> =
            std::collections::HashSet::new();
        let mut unique: Vec<[f32; 3]> = Vec::new();
        for p in parts.iter().flat_map(|part| part.iter()) {
            if seen.insert((p[0].to_bits(), p[1].to_bits(), p[2].to_bits())) {
                unique.push(*p);
            }
        }
        unique.sort_by(|a, b| {
            (a[0].to_bits(), a[1].to_bits(), a[2].to_bits()).cmp(&(
                b[0].to_bits(),
                b[1].to_bits(),
                b[2].to_bits(),
            ))
        });
        // Canonicals, bucketed by cell so the search stays local.
        let mut cells: std::collections::HashMap<(i32, i32, i32), Vec<[f32; 3]>> =
            std::collections::HashMap::new();
        let mut moved = std::collections::HashMap::new();
        let tol2 = tolerance * tolerance;
        for p in unique {
            let c = cell_of(p);
            let mut best: Option<[f32; 3]> = None;
            'search: for cx in c.0 - 1..=c.0 + 1 {
                for cy in c.1 - 1..=c.1 + 1 {
                    for cz in c.2 - 1..=c.2 + 1 {
                        let Some(bucket) = cells.get(&(cx, cy, cz)) else {
                            continue;
                        };
                        for q in bucket {
                            let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= tol2 {
                                best = Some(*q);
                                break 'search;
                            }
                        }
                    }
                }
            }
            match best {
                Some(q) => {
                    moved.insert((p[0].to_bits(), p[1].to_bits(), p[2].to_bits()), q);
                }
                None => cells.entry(c).or_default().push(p),
            }
        }
        Self { moved }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.moved.is_empty()
    }

    /// Move one part's vertices onto their canonicals and drop the triangles
    /// that collapse. Returns how many triangles were dropped: a triangle
    /// that vanishes here was under the tolerance across, so it drew nothing
    /// a viewer could see and was itself half of the crack.
    pub(crate) fn apply(&self, soup: Soup<'_>) -> usize {
        if self.moved.is_empty() {
            return 0;
        }
        let Soup { positions, uvs, normals, colors, indices } = soup;
        let n = positions.len();
        if uvs.len() != n {
            return 0;
        }
        for p in positions.iter_mut() {
            if let Some(q) = self
                .moved
                .get(&(p[0].to_bits(), p[1].to_bits(), p[2].to_bits()))
            {
                *p = *q;
            }
        }
        let before = indices.len() / 3;
        let mut kept = Vec::with_capacity(indices.len());
        for tri in indices.chunks_exact(3) {
            let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            if idx.iter().any(|i| *i >= n) {
                continue;
            }
            let p = [positions[idx[0]], positions[idx[1]], positions[idx[2]]];
            if area2(p[0], p[1], p[2]) <= f32::EPSILON {
                continue;
            }
            kept.extend_from_slice(tri);
        }
        *indices = kept;
        let _ = (normals, colors);
        before - indices.len() / 3
    }
}

fn one_pass(
    lookup: &VertexGrid,
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    mut normals: Option<&mut Vec<[f32; 3]>>,
    mut colors: Option<&mut Vec<[f32; 3]>>,
    indices: &mut Vec<u32>,
) -> usize {
    let in_pos = std::mem::take(positions);
    let in_uv = std::mem::take(uvs);
    let in_nrm = normals.as_mut().map(|v| std::mem::take(*v));
    let in_col = colors.as_mut().map(|v| std::mem::take(*v));
    let in_idx = std::mem::take(indices);

    let mut out = Soup3 {
        pos: Vec::with_capacity(in_pos.len()),
        uv: Vec::with_capacity(in_uv.len()),
        nrm: in_nrm.as_ref().map(|_| Vec::new()),
        col: in_col.as_ref().map(|_| Vec::new()),
        idx: Vec::with_capacity(in_idx.len()),
    };
    let mut splits = 0usize;
    // Work stack of (triangle, points still to be cut into each of its three
    // edges) and the scratch buffer the grid answers into.
    let mut work: Vec<([Corner; 3], [Vec<Corner>; 3])> = Vec::new();
    let mut on_edge: Vec<(f32, [f32; 3])> = Vec::new();

    for tri in in_idx.chunks_exact(3) {
        let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        if idx.iter().any(|i| *i >= in_pos.len()) {
            continue;
        }
        let corner = |i: usize| Corner {
            pos: in_pos[idx[i]],
            uv: in_uv[idx[i]],
            nrm: in_nrm.as_ref().map_or([0.0; 3], |n| n[idx[i]]),
            col: in_col.as_ref().map_or([1.0; 3], |c| c[idx[i]]),
        };
        let corners = [corner(0), corner(1), corner(2)];
        if area2(corners[0].pos, corners[1].pos, corners[2].pos) <= f32::EPSILON {
            // Degenerate input covers nothing: there is no crack to close
            // and no interior to cut it into.
            out.push_tri(&corners);
            continue;
        }
        // Collect the points to insert ONCE, against this triangle's own
        // three edges. The cuts below add chords, and a chord is never
        // itself searched: two vertices a couple of millimetres apart would
        // otherwise each keep landing inside a chord created for the other,
        // and the pass would split forever without closing anything.
        let mut pts: [Vec<Corner>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut inserted = 0usize;
        for k in 0..3 {
            let (a, b) = (corners[k], corners[(k + 1) % 3]);
            on_edge.clear();
            lookup.points_on(a.pos, b.pos, &mut on_edge);
            inserted += on_edge.len();
            pts[k] = on_edge
                .iter()
                .map(|(t, p)| Corner {
                    pos: *p,
                    uv: lerp2(a.uv, b.uv, *t),
                    nrm: lerp3(a.nrm, b.nrm, *t),
                    col: lerp3(a.col, b.col, *t),
                })
                .collect();
        }
        if inserted == 0 {
            // Untouched: the triangle goes out exactly as it came in.
            out.push_tri(&corners);
            continue;
        }
        splits += inserted;
        // Cut one point at a time, from the point to the opposite corner,
        // and hand each half the points that belong to its edges. This adds
        // no vertex the mesh did not already have, keeps the winding, and
        // ends with every inserted point a corner of two real triangles.
        work.clear();
        work.push((corners, pts));
        while let Some((t, mut edge_pts)) = work.pop() {
            let Some(k) = (0..3)
                .filter(|k| !edge_pts[*k].is_empty())
                .max_by_key(|k| edge_pts[*k].len())
            else {
                out.push_tri(&t);
                continue;
            };
            let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            let mut along = std::mem::take(&mut edge_pts[k]);
            // Cut at the middle point so long runs halve instead of peeling,
            // but prefer a point whose chord to the opposite corner is clean.
            // A chord that runs through some other vertex just moves the
            // T-junction inside the triangle, where the next pass has to
            // find it again — and when two vertices sit a couple of
            // millimetres apart, each keeps landing in the chord cut for the
            // other and the mesh grows without the count ever falling.
            let middle = along.len() / 2;
            let mut choice = middle;
            for step in 0..along.len() {
                let i = if step % 2 == 0 {
                    middle + step / 2
                } else {
                    match middle.checked_sub(step.div_ceil(2)) {
                        Some(i) => i,
                        None => continue,
                    }
                };
                if i >= along.len() {
                    continue;
                }
                on_edge.clear();
                lookup.points_on(along[i].pos, c.pos, &mut on_edge);
                if on_edge.is_empty() {
                    choice = i;
                    break;
                }
            }
            let right = along.split_off(choice);
            let (mid, right) = right.split_first().expect("k has points");
            let (p_bc, p_ca) = (
                std::mem::take(&mut edge_pts[(k + 1) % 3]),
                std::mem::take(&mut edge_pts[(k + 2) % 3]),
            );
            work.push(([a, *mid, c], [along, Vec::new(), p_ca]));
            work.push(([*mid, b, c], [right.to_vec(), p_bc, Vec::new()]));
        }
    }

    *positions = out.pos;
    *uvs = out.uv;
    *indices = out.idx;
    if let (Some(dst), Some(src)) = (normals, out.nrm) {
        *dst = src;
    }
    if let (Some(dst), Some(src)) = (colors, out.col) {
        *dst = src;
    }
    splits
}

#[derive(Clone, Copy)]
struct Corner {
    pos: [f32; 3],
    uv: [f32; 2],
    nrm: [f32; 3],
    col: [f32; 3],
}

/// Owned output buffers; `nrm` / `col` exist only when the input had them.
struct Soup3 {
    pos: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    nrm: Option<Vec<[f32; 3]>>,
    col: Option<Vec<[f32; 3]>>,
    idx: Vec<u32>,
}

impl Soup3 {
    fn push_tri(&mut self, tri: &[Corner; 3]) {
        let base = self.pos.len() as u32;
        for c in tri {
            self.pos.push(c.pos);
            self.uv.push(c.uv);
            if let Some(n) = &mut self.nrm {
                n.push(c.nrm);
            }
            if let Some(col) = &mut self.col {
                col.push(c.col);
            }
        }
        self.idx.extend_from_slice(&[base, base + 1, base + 2]);
    }
}

fn mean2(v: [[f32; 2]; 3]) -> [f32; 2] {
    [
        (v[0][0] + v[1][0] + v[2][0]) / 3.0,
        (v[0][1] + v[1][1] + v[2][1]) / 3.0,
    ]
}

fn mean3(v: [[f32; 3]; 3]) -> [f32; 3] {
    [
        (v[0][0] + v[1][0] + v[2][0]) / 3.0,
        (v[0][1] + v[1][1] + v[2][1]) / 3.0,
        (v[0][2] + v[1][2] + v[2][2]) / 3.0,
    ]
}

fn lerp2(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Twice the triangle area (the cross product's length).
fn area2(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// Unique vertex positions, bucketed by metre cell.
struct VertexGrid {
    cells: std::collections::HashMap<(i32, i32, i32), Vec<[f32; 3]>>,
}

impl VertexGrid {
    fn build(positions: &[[f32; 3]]) -> Self {
        Self::build_many(&[positions])
    }

    fn build_many(parts: &[&[[f32; 3]]]) -> Self {
        let total: usize = parts.iter().map(|part| part.len()).sum();
        let mut seen: std::collections::HashSet<(u32, u32, u32)> =
            std::collections::HashSet::with_capacity(total);
        let mut cells: std::collections::HashMap<(i32, i32, i32), Vec<[f32; 3]>> =
            std::collections::HashMap::new();
        for p in parts.iter().flat_map(|part| part.iter()) {
            if !seen.insert((p[0].to_bits(), p[1].to_bits(), p[2].to_bits())) {
                continue;
            }
            cells.entry(cell_of(*p)).or_default().push(*p);
        }
        Self { cells }
    }

    /// Vertices lying strictly inside the segment `a`..`b`, as
    /// `(t, position)` sorted along it. Endpoints are excluded — they are
    /// already shared.
    fn points_on(&self, a: [f32; 3], b: [f32; 3], out: &mut Vec<(f32, [f32; 3])>) {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if len2 <= ON_EDGE * ON_EDGE {
            return;
        }
        let len = len2.sqrt();
        let lo = cell_of([a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])]);
        let hi = cell_of([a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]);
        for cx in lo.0 - 1..=hi.0 + 1 {
            for cy in lo.1 - 1..=hi.1 + 1 {
                for cz in lo.2 - 1..=hi.2 + 1 {
                    let Some(bucket) = self.cells.get(&(cx, cy, cz)) else {
                        continue;
                    };
                    for p in bucket {
                        let w = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
                        let t = (w[0] * d[0] + w[1] * d[1] + w[2] * d[2]) / len2;
                        // Far enough from both ends to be an interior point
                        // rather than the shared endpoint itself.
                        if t * len <= MIN_FROM_END || (1.0 - t) * len <= MIN_FROM_END {
                            continue;
                        }
                        let perp = [w[0] - d[0] * t, w[1] - d[1] * t, w[2] - d[2] * t];
                        if perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]
                            > ON_EDGE * ON_EDGE
                        {
                            continue;
                        }
                        out.push((t, *p));
                    }
                }
            }
        }
        out.sort_by(|x, y| x.0.total_cmp(&y.0));
        // Two candidates closer together than one source unit are the same
        // cut: keeping both would leave a sliver microns wide between them,
        // which is worse than the crack either one closes. `dedup_by` keeps
        // the earlier of each pair, so this is a single sweep along the edge.
        out.dedup_by(|x, y| (x.0 - y.0).abs() * len <= MIN_FROM_END);
    }
}

fn cell_of(p: [f32; 3]) -> (i32, i32, i32) {
    (
        (p[0] / CELL).floor() as i32,
        (p[1] / CELL).floor() as i32,
        (p[2] / CELL).floor() as i32,
    )
}

/// How many vertices still sit strictly inside another triangle's edge,
/// counted with the same rule the weld uses. Test-only: it answers "did the
/// pass actually run over this mesh", which is a different question from the
/// independent 2D-per-plane audit the Doom map test runs.
#[cfg(test)]
pub(crate) fn t_junctions_left(parts: &[(&[[f32; 3]], &[u32])]) -> usize {
    let positions: Vec<&[[f32; 3]]> = parts.iter().map(|(p, _)| *p).collect();
    let grid = VertexGrid::build_many(&positions);
    let mut hits = 0usize;
    let mut on = Vec::new();
    for (pos, idx) in parts {
        for tri in idx.chunks_exact(3) {
            let p = [
                pos[tri[0] as usize],
                pos[tri[1] as usize],
                pos[tri[2] as usize],
            ];
            if area2(p[0], p[1], p[2]) <= f32::EPSILON {
                continue;
            }
            for k in 0..3 {
                on.clear();
                grid.points_on(p[k], p[(k + 1) % 3], &mut on);
                hits += on.len();
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two quads meeting along x = 1, the right one split in half at z = 0.5
    /// and the left one not: the classic T-junction, one vertex sitting in
    /// the middle of the left quad's right edge.
    fn t_fixture() -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
        let pos = vec![
            // left quad (0..1 in x, 0..1 in z), two triangles
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            // right strip, lower half (z 0..0.5)
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 0.0, 0.5],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.5],
            [1.0, 0.0, 0.5],
            // right strip, upper half (z 0.5..1)
            [1.0, 0.0, 0.5],
            [2.0, 0.0, 0.5],
            [2.0, 0.0, 1.0],
            [1.0, 0.0, 0.5],
            [2.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        let uv = pos.iter().map(|p| [p[0], p[2]]).collect();
        let idx = (0..pos.len() as u32).collect();
        (pos, uv, idx)
    }

    fn t_junction_count(pos: &[[f32; 3]], idx: &[u32]) -> usize {
        t_junctions_left(&[(pos, idx)])
    }

    fn area_sum(pos: &[[f32; 3]], idx: &[u32]) -> f32 {
        idx.chunks_exact(3)
            .map(|t| {
                area2(
                    pos[t[0] as usize],
                    pos[t[1] as usize],
                    pos[t[2] as usize],
                ) / 2.0
            })
            .sum()
    }

    #[test]
    fn a_vertex_inside_a_neighbours_edge_is_welded_in() {
        let (mut pos, mut uv, mut idx) = t_fixture();
        assert_eq!(t_junction_count(&pos, &idx), 1, "the fixture has one T");
        let before = area_sum(&pos, &idx);
        let split = split_t_junctions(Soup {
            positions: &mut pos,
            uvs: &mut uv,
            normals: None,
            colors: None,
            indices: &mut idx,
        });
        assert_eq!(split, 1, "exactly the one vertex is inserted");
        assert_eq!(t_junction_count(&pos, &idx), 0, "no T-junction survives");
        // Splitting only re-cuts existing surface: the covered area is the
        // same, so nothing was dropped and nothing was double-covered.
        assert!(
            (area_sum(&pos, &idx) - before).abs() < 1e-5,
            "area changed: {before} -> {}",
            area_sum(&pos, &idx)
        );
    }

    #[test]
    fn uvs_and_colours_follow_the_edge_they_are_inserted_on() {
        let (mut pos, mut uv, mut idx) = t_fixture();
        // Colour ramps with z so the interpolated value is checkable.
        let mut col: Vec<[f32; 3]> = pos.iter().map(|p| [p[2], 0.0, 0.0]).collect();
        split_t_junctions(Soup {
            positions: &mut pos,
            uvs: &mut uv,
            normals: None,
            colors: Some(&mut col),
            indices: &mut idx,
        });
        assert_eq!(col.len(), pos.len());
        assert_eq!(uv.len(), pos.len());
        for (p, (uv, col)) in pos.iter().zip(uv.iter().zip(col.iter())) {
            assert!((uv[0] - p[0]).abs() < 1e-5, "u tracks x at {p:?}: {uv:?}");
            assert!((uv[1] - p[2]).abs() < 1e-5, "v tracks z at {p:?}: {uv:?}");
            assert!((col[0] - p[2]).abs() < 1e-5, "colour tracks z at {p:?}");
        }
    }

    #[test]
    fn a_clean_mesh_is_left_exactly_alone() {
        let mut pos = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
        ];
        let mut uv = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let mut idx = vec![0, 1, 2];
        let (before_pos, before_idx) = (pos.clone(), idx.clone());
        let split = split_t_junctions(Soup {
            positions: &mut pos,
            uvs: &mut uv,
            normals: None,
            colors: None,
            indices: &mut idx,
        });
        assert_eq!(split, 0);
        assert_eq!(pos, before_pos);
        assert_eq!(idx, before_idx);
    }

    /// A wall standing on a floor edge is the same question in 3D: the
    /// wall's own split point sits inside the floor triangle's edge.
    #[test]
    fn a_wall_split_welds_into_the_floor_edge_below_it() {
        let mut pos = vec![
            // floor triangle spanning x 0..2 along z = 0
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            // wall standing on x 0..1 of that same edge, so x = 1, y = 0 is
            // a vertex sitting inside the floor's 0..2 edge
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let mut uv = vec![[0.0; 2]; pos.len()];
        let mut idx: Vec<u32> = (0..pos.len() as u32).collect();
        let split = split_t_junctions(Soup {
            positions: &mut pos,
            uvs: &mut uv,
            normals: None,
            colors: None,
            indices: &mut idx,
        });
        assert_eq!(split, 1);
        assert_eq!(t_junction_count(&pos, &idx), 0);
    }

    // -----------------------------------------------------------------
    // The corner merge
    // -----------------------------------------------------------------

    /// A crack the splitter cannot close: two corners a hair apart on the
    /// same seam. The merge snaps one onto the other, and the seam shuts.
    #[test]
    fn two_corners_a_hair_apart_become_one() {
        // Corner B is 2/256 m from corner A along z — inside the tolerance,
        // outside `MIN_FROM_END`, which is exactly the residual shape.
        let a = [1.0f32, 0.0, 0.0];
        let b = [1.0f32, 0.0, 2.0 / 256.0];
        let mut left = vec![[0.0, 0.0, 0.0], a, [0.0, 0.0, 1.0]];
        let mut right = vec![b, [2.0, 0.0, 0.0], [2.0, 0.0, 1.0]];
        let merge = Merge::from_parts(&[&left[..], &right[..]], MERGE_TOLERANCE);
        assert!(!merge.is_empty(), "a pair this close must merge");
        let mut lu = vec![[0.0; 2]; left.len()];
        let mut ru = vec![[0.0; 2]; right.len()];
        let mut li: Vec<u32> = (0..3).collect();
        let mut ri: Vec<u32> = (0..3).collect();
        merge.apply(Soup {
            positions: &mut left,
            uvs: &mut lu,
            normals: None,
            colors: None,
            indices: &mut li,
        });
        merge.apply(Soup {
            positions: &mut right,
            uvs: &mut ru,
            normals: None,
            colors: None,
            indices: &mut ri,
        });
        // Both parts now name ONE position where they had two.
        let shared: Vec<[f32; 3]> = left
            .iter()
            .filter(|p| right.contains(p))
            .copied()
            .collect();
        assert_eq!(shared.len(), 1, "left {left:?} right {right:?}");
        // The seam is now one position, and it is one of the two the map
        // authored — a merge never invents a coordinate between them.
        assert!(shared[0] == a || shared[0] == b, "{shared:?}");
    }

    /// Corners further apart than the tolerance are the map, not a defect,
    /// and must be left exactly where the author put them.
    #[test]
    fn honest_geometry_is_never_moved() {
        let pos = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            // A whole source unit past the tolerance.
            [1.0 + 4.0 / 256.0, 0.0, 0.0],
        ];
        let merge = Merge::from_parts(&[&pos[..]], MERGE_TOLERANCE);
        assert!(merge.is_empty(), "nothing here is within the tolerance");
        let mut p2 = pos.clone();
        let mut uv = vec![[0.0; 2]; p2.len()];
        let mut idx: Vec<u32> = vec![0, 1, 2];
        assert_eq!(
            merge.apply(Soup {
                positions: &mut p2,
                uvs: &mut uv,
                normals: None,
                colors: None,
                indices: &mut idx,
            }),
            0
        );
        assert_eq!(p2, pos);
    }

    /// A triangle that collapses IS the crack: it was under the tolerance
    /// across, so it drew nothing, and leaving a degenerate behind would
    /// hand the renderer a NaN normal.
    #[test]
    fn a_sliver_that_collapses_is_dropped() {
        let mut pos = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0 / 256.0],
            [1.0, 0.0, 0.0],
            [3.0, 0.0, 0.0],
            [3.0, 0.0, 1.0],
        ];
        let merge = Merge::from_parts(&[&pos[..]], MERGE_TOLERANCE);
        let mut uv = vec![[0.0; 2]; pos.len()];
        let mut idx: Vec<u32> = (0..6).collect();
        let dropped = merge.apply(Soup {
            positions: &mut pos,
            uvs: &mut uv,
            normals: None,
            colors: None,
            indices: &mut idx,
        });
        assert_eq!(dropped, 1, "the sliver went, the real triangle stayed");
        assert_eq!(idx.len(), 3);
    }

    /// Reruns must be byte-identical, so the table cannot depend on the
    /// order the parts arrive in.
    #[test]
    fn the_merge_table_does_not_depend_on_part_order() {
        let a = vec![[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let b = vec![[1.0f32, 0.0, 2.0 / 256.0], [5.0, 0.0, 0.0]];
        let one = Merge::from_parts(&[&a[..], &b[..]], MERGE_TOLERANCE);
        let two = Merge::from_parts(&[&b[..], &a[..]], MERGE_TOLERANCE);
        let mut ka: Vec<_> = one.moved.iter().map(|(k, v)| (*k, *v)).collect();
        let mut kb: Vec<_> = two.moved.iter().map(|(k, v)| (*k, *v)).collect();
        ka.sort_by_key(|(k, _)| *k);
        kb.sort_by_key(|(k, _)| *k);
        assert_eq!(ka, kb);
        assert_eq!(ka.len(), 1);
    }
}
