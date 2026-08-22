//! Mesh post-processing for generated (dual-grid / retopo) surfaces, ported
//! from trellis.cpp's CPU postprocess (uv_bake.cpp + decimate_qem.cpp — the
//! reference's cumesh chain re-derived on CPU):
//!
//! - [`weld_vertices`]: merge epsilon-duplicate vertices (dual-grid decoders
//!   emit hairline cracks at shared cell corners).
//! - [`fill_small_holes`]: fan-fill small boundary loops (directed walk +
//!   winding-agnostic fallback).
//! - [`drop_small_components`]: remove floater components below a fraction
//!   of the largest component's face count.
//! - [`unify_face_orientations`]: propagate coherent winding across every
//!   manifold component without incorrectly flipping nested inner shells.
//! - [`decimate_qem`]: CuMesh-faithful Garland-Heckbert QEM edge collapse
//!   (edge-length + skinny-triangle penalties, flip rejection, boundary
//!   weighting, independent-set rounds, threshold ladder).
//!
//! All functions operate on `positions: Vec<[f32; 3]>` + flat `indices:
//! Vec<u32>` triangles, in place where the crib does.

/// Undirected edge key: (min, max) packed into a u64.
#[inline]
fn ekey(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshTopologyAudit {
    pub faces: usize,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    pub inconsistent_edges: usize,
    pub signed_volume: f64,
}

#[derive(Clone, Copy)]
struct FaceEdge {
    key: u64,
    face: u32,
    /// Direction relative to the edge's sorted (low -> high) endpoints.
    direction: i8,
}

fn face_edges(indices: &[u32]) -> Vec<FaceEdge> {
    let mut edges = Vec::with_capacity(indices.len());
    for (face, tri) in indices.chunks_exact(3).enumerate() {
        for corner in 0..3 {
            let a = tri[corner];
            let b = tri[(corner + 1) % 3];
            if a == b {
                continue;
            }
            edges.push(FaceEdge {
                key: ekey(a, b),
                face: face as u32,
                direction: if a < b { 1 } else { -1 },
            });
        }
    }
    edges.sort_unstable_by_key(|edge| edge.key);
    edges
}

/// Count boundary, non-manifold, and same-direction shared edges. A coherent
/// oriented manifold uses every interior edge exactly twice in opposite
/// directions. Signed volume remains useful for diagnosing nested shells.
pub fn audit_mesh_topology(
    positions: &[[f32; 3]],
    indices: &[u32],
) -> MeshTopologyAudit {
    let edges = face_edges(indices);
    let mut audit = MeshTopologyAudit {
        faces: indices.len() / 3,
        ..Default::default()
    };
    let mut at = 0usize;
    while at < edges.len() {
        let mut end = at + 1;
        while end < edges.len() && edges[end].key == edges[at].key {
            end += 1;
        }
        match end - at {
            1 => audit.boundary_edges += 1,
            2 => {
                if edges[at].direction == edges[at + 1].direction {
                    audit.inconsistent_edges += 1;
                }
            }
            _ => audit.nonmanifold_edges += 1,
        }
        at = end;
    }
    for tri in indices.chunks_exact(3) {
        if tri.iter().any(|&index| index as usize >= positions.len()) {
            continue;
        }
        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let cross = [
            b[1] as f64 * c[2] as f64 - b[2] as f64 * c[1] as f64,
            b[2] as f64 * c[0] as f64 - b[0] as f64 * c[2] as f64,
            b[0] as f64 * c[1] as f64 - b[1] as f64 * c[0] as f64,
        ];
        audit.signed_volume +=
            (a[0] as f64 * cross[0] + a[1] as f64 * cross[1] + a[2] as f64 * cross[2])
                / 6.0;
    }
    audit
}

/// Make adjacent manifold faces traverse their shared edge in opposite
/// directions. Global component winding is deliberately preserved: a UDF
/// band has a positive-volume outer shell and negative-volume inner shell,
/// and forcing both positive breaks that valid nested-surface orientation.
/// Returns the number of faces whose winding changed.
pub fn unify_face_orientations(positions: &[[f32; 3]], indices: &mut [u32]) -> usize {
    let faces = indices.len() / 3;
    if faces == 0 {
        return 0;
    }
    let edges = face_edges(indices);
    let mut adjacency: Vec<Vec<(u32, bool)>> = vec![Vec::new(); faces];
    let mut at = 0usize;
    while at < edges.len() {
        let mut end = at + 1;
        while end < edges.len() && edges[end].key == edges[at].key {
            end += 1;
        }
        if end - at == 2 {
            let a = edges[at];
            let b = edges[at + 1];
            let opposite_flip = a.direction == b.direction;
            adjacency[a.face as usize].push((b.face, opposite_flip));
            adjacency[b.face as usize].push((a.face, opposite_flip));
        }
        at = end;
    }

    let mut flip = vec![-1i8; faces];
    for start in 0..faces {
        if flip[start] >= 0 {
            continue;
        }
        flip[start] = 0;
        let mut stack = vec![start as u32];
        while let Some(face) = stack.pop() {
            let state = flip[face as usize];
            for &(neighbor, opposite) in &adjacency[face as usize] {
                let required = state ^ i8::from(opposite);
                if flip[neighbor as usize] < 0 {
                    flip[neighbor as usize] = required;
                    stack.push(neighbor);
                }
            }
        }
    }

    let mut changed = 0usize;
    for (face, &should_flip) in flip.iter().enumerate() {
        if should_flip != 0 {
            indices.swap(face * 3 + 1, face * 3 + 2);
            changed += 1;
        }
    }
    let _ = positions;
    changed
}

/// Merge vertices within `step` of each other; faces are remapped in place.
/// Also collapses any triangle that welding turned into an exact duplicate
/// (see the doublet note below) or a degenerate `{a,a,b}` down to a single
/// kept copy, shrinking `indices` in place. Returns the number of duplicate
/// *vertices* removed (unchanged meaning; doublet triangles dropped are not
/// counted in this total).
pub fn weld_vertices(
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    step: f32,
) -> usize {
    weld_vertices_ctl(positions, indices, step, &mut |_, _| true)
}

/// Same as [`weld_vertices`]. `ctl(done, total)` returning false stops early
/// and leaves `positions`/`indices` unchanged.
pub fn weld_vertices_ctl(
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    step: f32,
    ctl: &mut impl FnMut(usize, usize) -> bool,
) -> usize {
    use std::collections::HashMap;
    let v = positions.len();
    let eps2 = step * step;
    let ckey = |x: i64, y: i64, z: i64| -> u64 {
        ((x as u64 & 0x3f_ffff) << 42) ^ ((y as u64 & 0x1f_ffff) << 21) ^ (z as u64 & 0x1f_ffff)
    };
    let mut cells: HashMap<u64, Vec<u32>> = HashMap::with_capacity(v * 2);
    let mut remap = vec![0u32; v];
    let mut kept: Vec<[f32; 3]> = Vec::with_capacity(v);
    let tick = (v / 20).max(1);
    if !ctl(0, v) {
        return 0;
    }
    for i in 0..v {
        if i % tick == 0 && !ctl(i, v) {
            return 0;
        }
        let p = positions[i];
        let cx = (p[0] / step).floor() as i64;
        let cy = (p[1] / step).floor() as i64;
        let cz = (p[2] / step).floor() as i64;
        let mut found: Option<u32> = None;
        'search: for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(list) = cells.get(&ckey(cx + dx, cy + dy, cz + dz)) {
                        for &c in list {
                            let q = kept[c as usize];
                            let e = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                            if e[0] * e[0] + e[1] * e[1] + e[2] * e[2] <= eps2 {
                                found = Some(c);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        let id = match found {
            Some(c) => c,
            None => {
                let c = kept.len() as u32;
                kept.push(p);
                cells.entry(ckey(cx, cy, cz)).or_default().push(c);
                c
            }
        };
        remap[i] = id;
    }
    for f in indices.iter_mut() {
        *f = remap[*f as usize];
    }
    let welded = v - kept.len();
    *positions = kept;

    // Fusing near-coincident vertices can also fuse a hairline-thin double
    // wall (front/back sheets of a thin feature, e.g. a nose bridge, ear, or
    // hat point) into a single set of shared vertices while leaving the two
    // independently-decoded triangles behind: an exact-duplicate triangle
    // pair, almost always opposite-wound since the two sheets face away
    // from each other, sitting at zero distance. `audit_mesh_topology`
    // cannot see this as a hole (each shared edge still looks like an
    // ordinary 2-face, opposite-direction pair) — but at render time the
    // two coincident triangles z-fight, and the reversed-normal one shades
    // dark, flipping per pixel into an isolated black-pixel pinhole on
    // exactly the thin, high-curvature spots (forehead/nose/hat) where
    // welding is most likely to fuse a double wall.
    //
    // Collapse a duplicate group (same vertex set, either winding) down to
    // one kept triangle ONLY when every one of its 3 edges has a genuine
    // neighbor beyond the group itself — i.e. dropping the extra copies
    // cannot strand an edge at count 1. Some of these duplicate pairs turn
    // out to be the *only* coverage of a hairline decoder crack (FaithC
    // issue #3): both sides of the crack independently plug the same tiny
    // gap, landing on identical welded vertices. Deduping those unconditionally
    // would trade a z-fighting doublet for an actual open hole, which
    // `fill_small_holes` mostly can't close (dangling, non-loop boundary
    // edges) — strictly worse. Leaving that subset as a doublet keeps
    // today's (already shipped) visual behavior unchanged there while still
    // removing every doublet that's provably redundant.
    let f = indices.len() / 3;
    let mut edge_count: HashMap<u64, u32> = HashMap::with_capacity(f * 3);
    for tri in indices.chunks_exact(3) {
        for j in 0..3 {
            let a = tri[j];
            let b = tri[(j + 1) % 3];
            if a != b {
                *edge_count.entry(ekey(a, b)).or_insert(0) += 1;
            }
        }
    }
    let mut groups: HashMap<[u32; 3], Vec<usize>> = HashMap::with_capacity(f);
    for r in 0..f {
        let tri = [indices[r * 3], indices[r * 3 + 1], indices[r * 3 + 2]];
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            continue; // degenerate rows are dropped unconditionally below
        }
        let mut key = tri;
        key.sort_unstable();
        groups.entry(key).or_default().push(r);
    }
    let mut drop = vec![false; f];
    for (key, rows) in &groups {
        if rows.len() < 2 {
            continue;
        }
        let edges = [ekey(key[0], key[1]), ekey(key[1], key[2]), ekey(key[0], key[2])];
        let safe = edges.iter().all(|e| {
            edge_count.get(e).copied().unwrap_or(0) as usize >= rows.len() + 1
        });
        if safe {
            for &r in &rows[1..] {
                drop[r] = true;
            }
        }
    }
    for r in 0..f {
        let tri = [indices[r * 3], indices[r * 3 + 1], indices[r * 3 + 2]];
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            drop[r] = true; // degenerate after welding: zero-area, always safe to drop
        }
    }
    let mut w = 0usize;
    for r in 0..f {
        if drop[r] {
            continue;
        }
        indices[w * 3] = indices[r * 3];
        indices[w * 3 + 1] = indices[r * 3 + 1];
        indices[w * 3 + 2] = indices[r * 3 + 2];
        w += 1;
    }
    indices.truncate(w * 3);

    let _ = ctl(v, v);
    welded
}

/// Fan-fill boundary loops of at most `max_loop` edges. Two passes: a
/// directed rim walk (keeps winding consistent with neighbors), then a
/// winding-agnostic pass over undirected boundary adjacency (simplification
/// tears flip boundary direction and never chain in the directed walk).
/// Returns the number of loops filled; `indices` grows by the fan faces.
pub fn fill_small_holes(indices: &mut Vec<u32>, max_loop: usize) -> usize {
    fill_small_holes_ctl(indices, max_loop, &mut |_, _| true)
}

/// Same as [`fill_small_holes`]. `ctl(done, total)` is called as the edge
/// maps build and boundary loops are walked (`total` = 4 phases × the item
/// count of each phase, monotone); returning false stops early — the fills
/// found so far stay applied.
pub fn fill_small_holes_ctl(
    indices: &mut Vec<u32>,
    max_loop: usize,
    ctl: &mut impl FnMut(usize, usize) -> bool,
) -> usize {
    use std::collections::HashMap;
    let f = indices.len() / 3;
    // Progress: phase 0 = directed edge map (f), 1 = directed loop walk
    // (starts), 2 = undirected edge map (f), 3 = undirected walk (starts).
    // Report on a per-4096-item cadence so the callback stays cheap.
    const CADENCE: usize = 4096;
    let phase_total = f.max(1);
    let total = 4 * phase_total;
    let mut report = |phase: usize, i: usize, n: usize, ctl: &mut dyn FnMut(usize, usize) -> bool| -> bool {
        if i % CADENCE != 0 && i + 1 != n {
            return true;
        }
        let frac = if n == 0 { 1.0 } else { i as f64 / n as f64 };
        ctl(phase * phase_total + (frac * phase_total as f64) as usize, total)
    };
    let dkey = |a: u32, b: u32| ((a as u64) << 32) | b as u64;
    let mut dir: HashMap<u64, u32> = HashMap::with_capacity(f * 3);
    for (ti, tri) in indices.chunks_exact(3).enumerate() {
        for j in 0..3 {
            *dir.entry(dkey(tri[j], tri[(j + 1) % 3])).or_insert(0) += 1;
        }
        if !report(0, ti, f, ctl) {
            return 0;
        }
    }
    // Directed boundary chains through unambiguous (out/in degree 1) verts.
    let mut nxt: HashMap<u32, u32> = HashMap::new();
    let mut outd: HashMap<u32, u32> = HashMap::new();
    let mut ind: HashMap<u32, u32> = HashMap::new();
    for (&k, &cnt) in &dir {
        let a = (k >> 32) as u32;
        let b = k as u32;
        if cnt == 1 && !dir.contains_key(&dkey(b, a)) {
            nxt.insert(b, a);
            *outd.entry(b).or_insert(0) += 1;
            *ind.entry(a).or_insert(0) += 1;
        }
    }
    let mut used: HashMap<u32, bool> = HashMap::new();
    let mut filled = 0usize;
    let starts: Vec<u32> = nxt.keys().copied().collect();
    let n_starts = starts.len();
    for (si, start) in starts.into_iter().enumerate() {
        if !report(1, si, n_starts, ctl) {
            return filled;
        }
        if used.get(&start).copied().unwrap_or(false)
            || outd.get(&start) != Some(&1)
            || ind.get(&start) != Some(&1)
        {
            continue;
        }
        let mut loop_verts = vec![start];
        let mut cur = start;
        let mut cycle = false;
        let mut clean = true;
        for _ in 0..=max_loop {
            let Some(&n) = nxt.get(&cur) else {
                clean = false;
                break;
            };
            if used.get(&cur).copied().unwrap_or(false) {
                clean = false;
                break;
            }
            cur = n;
            if outd.get(&cur) != Some(&1) || ind.get(&cur) != Some(&1) {
                clean = false;
                break;
            }
            if cur == start {
                cycle = true;
                break;
            }
            loop_verts.push(cur);
        }
        for &v in &loop_verts {
            used.insert(v, true);
        }
        if !clean || !cycle || loop_verts.len() < 3 || loop_verts.len() > max_loop {
            continue;
        }
        for i in 1..loop_verts.len() - 1 {
            indices.extend_from_slice(&[loop_verts[0], loop_verts[i], loop_verts[i + 1]]);
        }
        filled += 1;
    }
    // Winding-agnostic pass over undirected boundary adjacency.
    {
        let f2 = indices.len() / 3;
        let mut und: HashMap<u64, u32> = HashMap::with_capacity(f2 * 3);
        for (ti, tri) in indices.chunks_exact(3).enumerate() {
            for j in 0..3 {
                *und.entry(ekey(tri[j], tri[(j + 1) % 3])).or_insert(0) += 1;
            }
            if !report(2, ti, f2, ctl) {
                return filled;
            }
        }
        let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&k, &cnt) in &und {
            if cnt != 1 {
                continue;
            }
            let a = (k >> 32) as u32;
            let b = k as u32;
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        let mut used2: HashMap<u32, bool> = HashMap::new();
        let starts: Vec<u32> = adj.keys().copied().collect();
        let n_starts = starts.len();
        let mut fans: Vec<u32> = Vec::new();
        for (si, start) in starts.into_iter().enumerate() {
            if !report(3, si, n_starts, ctl) {
                break;
            }
            if used2.get(&start).copied().unwrap_or(false) {
                continue;
            }
            let Some(nbrs) = adj.get(&start) else { continue };
            if nbrs.len() != 2 {
                continue;
            }
            let mut loop_verts = vec![start];
            let mut prev = start;
            let mut cur = nbrs[0];
            let mut cycle = false;
            let mut clean = true;
            for _ in 0..=max_loop {
                let Some(n) = adj.get(&cur) else {
                    clean = false;
                    break;
                };
                if n.len() != 2 || used2.get(&cur).copied().unwrap_or(false) {
                    clean = false;
                    break;
                }
                if cur == start {
                    cycle = true;
                    break;
                }
                loop_verts.push(cur);
                let nx = if n[0] == prev { n[1] } else { n[0] };
                prev = cur;
                cur = nx;
            }
            for &v in &loop_verts {
                used2.insert(v, true);
            }
            if !clean || !cycle || loop_verts.len() < 3 || loop_verts.len() > max_loop {
                continue;
            }
            for i in 1..loop_verts.len() - 1 {
                fans.extend_from_slice(&[loop_verts[0], loop_verts[i], loop_verts[i + 1]]);
            }
            filled += 1;
        }
        indices.extend_from_slice(&fans);
    }
    filled
}

/// Drop connected components whose face count is below `frac` of the largest
/// component's. Compacts positions + indices in place; returns dropped count.
pub fn drop_small_components(
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    frac: f32,
) -> usize {
    drop_small_components_ctl(positions, indices, frac, &mut |_, _| true)
}

/// Same as [`drop_small_components`]. `ctl(done, total)` reports the
/// union-find (`total` = 3 × faces); returning false stops early with the
/// mesh unchanged.
pub fn drop_small_components_ctl(
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    frac: f32,
    ctl: &mut impl FnMut(usize, usize) -> bool,
) -> usize {
    let v = positions.len();
    let f = indices.len() / 3;
    if v == 0 || f == 0 {
        return 0;
    }
    const CADENCE: usize = 8192;
    let total = 3 * f;
    let mut par: Vec<u32> = (0..v as u32).collect();
    fn find(par: &mut [u32], mut x: u32) -> u32 {
        while par[x as usize] != x {
            par[x as usize] = par[par[x as usize] as usize];
            x = par[x as usize];
        }
        x
    }
    for (ti, tri) in indices.chunks_exact(3).enumerate() {
        let ra = find(&mut par, tri[0]);
        let rb = find(&mut par, tri[1]);
        if ra != rb {
            par[ra as usize] = rb;
        }
        let rb = find(&mut par, tri[1]);
        let rc = find(&mut par, tri[2]);
        if rb != rc {
            par[rb as usize] = rc;
        }
        if ti % CADENCE == 0 && !ctl(ti, total) {
            return 0;
        }
    }
    let mut count = vec![0u32; v];
    for (ti, tri) in indices.chunks_exact(3).enumerate() {
        count[find(&mut par, tri[0]) as usize] += 1;
        if ti % CADENCE == 0 && !ctl(f + ti, total) {
            return 0;
        }
    }
    let maxfc = count.iter().copied().max().unwrap_or(0);
    let thresh = (frac * maxfc as f32) as u32;
    let mut dropped = 0usize;
    for &c in &count {
        if c > 0 && c < thresh {
            dropped += 1;
        }
    }
    if dropped == 0 {
        return 0;
    }
    let mut kept: Vec<u32> = Vec::with_capacity(indices.len());
    for (ti, tri) in indices.chunks_exact(3).enumerate() {
        if count[find(&mut par, tri[0]) as usize] >= thresh {
            kept.extend_from_slice(tri);
        }
        if ti % CADENCE == 0 && !ctl(2 * f + ti, total) {
            return 0;
        }
    }
    let _ = ctl(total, total);
    // Compact to referenced vertices.
    let mut remap = vec![u32::MAX; v];
    let mut nv: Vec<[f32; 3]> = Vec::new();
    for idx in kept.iter_mut() {
        let old = *idx as usize;
        if remap[old] == u32::MAX {
            remap[old] = nv.len() as u32;
            nv.push(positions[old]);
        }
        *idx = remap[old];
    }
    *positions = nv;
    *indices = kept;
    dropped
}

/// Symmetric 4x4 plane quadric (upper triangle, 10 floats).
#[derive(Clone, Copy, Default)]
struct Qem {
    e: [f32; 10],
}

impl Qem {
    #[inline]
    fn add_plane(&mut self, a: f32, b: f32, c: f32, d: f32) {
        let e = &mut self.e;
        e[0] += a * a;
        e[1] += a * b;
        e[2] += a * c;
        e[3] += a * d;
        e[4] += b * b;
        e[5] += b * c;
        e[6] += b * d;
        e[7] += c * c;
        e[8] += c * d;
        e[9] += d * d;
    }
    #[inline]
    fn add(&self, o: &Qem) -> Qem {
        let mut r = Qem::default();
        for k in 0..10 {
            r.e[k] = self.e[k] + o.e[k];
        }
        r
    }
    #[inline]
    fn evaluate(&self, x: f32, y: f32, z: f32) -> f32 {
        let e = &self.e;
        e[0] * x * x
            + 2.0 * e[1] * x * y
            + 2.0 * e[2] * x * z
            + 2.0 * e[3] * x
            + e[4] * y * y
            + 2.0 * e[5] * y * z
            + 2.0 * e[6] * y
            + e[7] * z * z
            + 2.0 * e[8] * z
            + e[9]
    }
}

#[inline]
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn pack_cost(id: u32, c: f32) -> u64 {
    ((c.to_bits() as u64) << 32) | id as u64
}

/// One independent-set collapse round (crib: simplify_round). Mutates the
/// buffers; returns nothing — the caller reads the new lengths.
fn simplify_round(
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    lam_len: f32,
    lam_skinny: f32,
    thresh: f32,
) {
    let v = positions.len();
    let f = indices.len() / 3;

    // vertex -> incident faces (CSR, counting sort)
    let mut off = vec![0u32; v + 1];
    for &i in indices.iter() {
        off[i as usize + 1] += 1;
    }
    for i in 0..v {
        off[i + 1] += off[i];
    }
    let mut v2f = vec![0u32; off[v] as usize];
    {
        let mut cur = off.clone();
        for (fi, tri) in indices.chunks_exact(3).enumerate() {
            for &vi in tri {
                v2f[cur[vi as usize] as usize] = fi as u32;
                cur[vi as usize] += 1;
            }
        }
    }

    // unique undirected edges + boundary verts, sort-based (no hashing)
    let mut ekeys: Vec<u64> = Vec::with_capacity(f * 3);
    for tri in indices.chunks_exact(3) {
        ekeys.push(ekey(tri[0], tri[1]));
        ekeys.push(ekey(tri[1], tri[2]));
        ekeys.push(ekey(tri[2], tri[0]));
    }
    ekeys.sort_unstable();
    let mut edges: Vec<u64> = Vec::with_capacity(ekeys.len() / 2);
    let mut boundary = vec![false; v];
    {
        let mut i = 0;
        while i < ekeys.len() {
            let k = ekeys[i];
            let mut j = i + 1;
            while j < ekeys.len() && ekeys[j] == k {
                j += 1;
            }
            edges.push(k);
            if j - i == 1 {
                boundary[(k >> 32) as usize] = true;
                boundary[(k & 0xffff_ffff) as usize] = true;
            }
            i = j;
        }
    }
    let ecount = edges.len();

    // per-vertex quadrics from incident face planes
    let mut qem = vec![Qem::default(); v];
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let mut n = cross3(sub3(b, a), sub3(c, a));
        let ln = dot3(n, n).sqrt();
        if ln > 1e-20 {
            n = [n[0] / ln, n[1] / ln, n[2] / ln];
        }
        let d = -dot3(n, a);
        let _ = fi;
        for &vi in tri {
            qem[vi as usize].add_plane(n[0], n[1], n[2], d);
        }
    }

    // Shape metric + flip rejection over faces incident to `keep` (skipping
    // those that also touch `other`, which the collapse removes).
    let process = |keep: u32, other: u32, vn: [f32; 3], skinny: &mut f32, ntri: &mut u32| -> bool {
        let s = off[keep as usize] as usize;
        let e = off[keep as usize + 1] as usize;
        for &fi in &v2f[s..e] {
            let tri = &indices[fi as usize * 3..fi as usize * 3 + 3];
            if tri.contains(&other) {
                continue;
            }
            let a = positions[tri[0] as usize];
            let b = positions[tri[1] as usize];
            let c = positions[tri[2] as usize];
            let na = if tri[0] == keep { vn } else { a };
            let nb = if tri[1] == keep { vn } else { b };
            let nc = if tri[2] == keep { vn } else { c };
            let on = cross3(sub3(b, a), sub3(c, a));
            let ne1 = sub3(nb, na);
            let ne2 = sub3(nc, na);
            let nn = cross3(ne1, ne2);
            if dot3(on, nn) < 0.0 {
                return false; // flip
            }
            let narea = 0.5 * dot3(nn, nn).sqrt();
            let mut denom = dot3(sub3(nc, nb), sub3(nc, nb)) + dot3(ne1, ne1) + dot3(ne2, ne2);
            if denom < 1e-12 {
                denom = 1e-12;
            }
            let sm = (4.0 * 1.732_050_8 * narea / denom).clamp(0.0, 1.0);
            *skinny += 1.0 - sm;
            *ntri += 1;
        }
        true
    };

    // edge cost + collapse target
    let mut cost = vec![0f32; ecount];
    let mut vnew = vec![[0f32; 3]; ecount];
    for (t, &key) in edges.iter().enumerate() {
        let e0 = (key >> 32) as u32;
        let e1 = (key & 0xffff_ffff) as u32;
        let v0 = positions[e0 as usize];
        let v1 = positions[e1 as usize];
        let w0 = if boundary[e0 as usize] && !boundary[e1 as usize] {
            1.0
        } else if !boundary[e0 as usize] && boundary[e1 as usize] {
            0.0
        } else {
            0.5
        };
        let vm = [
            v0[0] * w0 + v1[0] * (1.0 - w0),
            v0[1] * w0 + v1[1] * (1.0 - w0),
            v0[2] * w0 + v1[2] * (1.0 - w0),
        ];
        vnew[t] = vm;
        let el2 = dot3(sub3(v1, v0), sub3(v1, v0));
        let mut c = qem[e0 as usize]
            .add(&qem[e1 as usize])
            .evaluate(vm[0], vm[1], vm[2]);
        c += lam_len * el2;
        let mut skinny = 0f32;
        let mut ntri = 0u32;
        let ok = process(e0, e1, vm, &mut skinny, &mut ntri)
            && process(e1, e0, vm, &mut skinny, &mut ntri);
        if !ok {
            cost[t] = f32::INFINITY;
            continue;
        }
        if ntri > 0 {
            skinny /= ntri as f32;
        }
        c += lam_skinny * skinny * el2;
        cost[t] = c;
    }

    // propagate min (cost, id) to every face touching either endpoint
    let mut prop = vec![u64::MAX; f];
    for t in 0..ecount {
        let p = pack_cost(t as u32, cost[t]);
        let e0 = (edges[t] >> 32) as usize;
        let e1 = (edges[t] & 0xffff_ffff) as usize;
        for &fi in &v2f[off[e0] as usize..off[e0 + 1] as usize] {
            if p < prop[fi as usize] {
                prop[fi as usize] = p;
            }
        }
        for &fi in &v2f[off[e1] as usize..off[e1 + 1] as usize] {
            if p < prop[fi as usize] {
                prop[fi as usize] = p;
            }
        }
    }

    // collapse winners under threshold
    let mut vdead = vec![false; v];
    let mut fdead = vec![false; f];
    for t in 0..ecount {
        if !(cost[t] <= thresh) {
            continue;
        }
        let p = pack_cost(t as u32, cost[t]);
        let e0 = (edges[t] >> 32) as u32;
        let e1 = (edges[t] & 0xffff_ffff) as u32;
        let mut own = true;
        for &fi in &v2f[off[e0 as usize] as usize..off[e0 as usize + 1] as usize] {
            if prop[fi as usize] != p {
                own = false;
                break;
            }
        }
        if own {
            for &fi in &v2f[off[e1 as usize] as usize..off[e1 as usize + 1] as usize] {
                if prop[fi as usize] != p {
                    own = false;
                    break;
                }
            }
        }
        if !own {
            continue;
        }
        positions[e0 as usize] = vnew[t];
        vdead[e1 as usize] = true;
        for &fi in &v2f[off[e0 as usize] as usize..off[e0 as usize + 1] as usize] {
            let tri = &indices[fi as usize * 3..fi as usize * 3 + 3];
            if tri.contains(&e1) {
                fdead[fi as usize] = true;
            }
        }
        for &fi in &v2f[off[e1 as usize] as usize..off[e1 as usize + 1] as usize] {
            let base = fi as usize * 3;
            for k in 0..3 {
                if indices[base + k] == e1 {
                    indices[base + k] = e0;
                }
            }
        }
    }

    // compact
    let mut vmap = vec![u32::MAX; v];
    let mut nv: Vec<[f32; 3]> = Vec::with_capacity(v);
    for i in 0..v {
        if !vdead[i] {
            vmap[i] = nv.len() as u32;
            nv.push(positions[i]);
        }
    }
    let mut nf: Vec<u32> = Vec::with_capacity(indices.len());
    for (fi, tri) in indices.chunks_exact(3).enumerate() {
        if fdead[fi] {
            continue;
        }
        let a = vmap[tri[0] as usize];
        let b = vmap[tri[1] as usize];
        let c = vmap[tri[2] as usize];
        if a == u32::MAX || b == u32::MAX || c == u32::MAX || a == b || b == c || a == c {
            continue;
        }
        nf.extend_from_slice(&[a, b, c]);
    }
    *positions = nv;
    *indices = nf;
}

/// CuMesh-faithful QEM edge-collapse simplification to ~`target_faces`.
/// Returns (positions, indices) compacted to referenced vertices.
pub fn decimate_qem(
    in_positions: &[[f32; 3]],
    in_indices: &[u32],
    target_faces: usize,
) -> (Vec<[f32; 3]>, Vec<u32>) {
    decimate_qem_ctl(in_positions, in_indices, target_faces, &mut |_, _| true)
        .expect("uncancellable decimate")
}

/// [`decimate_qem`] with round-level control: `ctl(round, faces_now)` runs
/// before every collapse round; returning false aborts with None (service
/// job cancellation) — it doubles as the progress tick.
pub fn decimate_qem_ctl(
    in_positions: &[[f32; 3]],
    in_indices: &[u32],
    target_faces: usize,
    ctl: &mut dyn FnMut(usize, usize) -> bool,
) -> Option<(Vec<[f32; 3]>, Vec<u32>)> {
    let mut positions = in_positions.to_vec();
    let mut indices = in_indices.to_vec();
    if indices.len() / 3 <= target_faces {
        return Some((positions, indices));
    }
    let mut thresh = 1e-8f32;
    let (lam_len, lam_skinny) = (1e-2f32, 1e-3f32);
    let mut prev_f = indices.len() / 3;
    let mut stalls = 0;
    for round in 0..400 {
        if indices.len() / 3 <= target_faces {
            break;
        }
        if !ctl(round, indices.len() / 3) {
            return None;
        }
        simplify_round(&mut positions, &mut indices, lam_len, lam_skinny, thresh);
        let f = indices.len() / 3;
        if f <= target_faces {
            break;
        }
        let removed = prev_f.saturating_sub(f);
        if removed == 0 {
            stalls += 1;
            if stalls >= 2 {
                thresh *= 10.0;
                stalls = 0;
            }
        } else {
            stalls = 0;
            if (removed as f32) / (prev_f as f32) < 1e-2 {
                thresh *= 10.0;
            }
        }
        prev_f = f;
        if thresh > 1e12 {
            break;
        }
    }
    // final compaction to referenced vertices
    let mut used = vec![u32::MAX; positions.len()];
    let mut nv: Vec<[f32; 3]> = Vec::new();
    let mut out_idx = indices.clone();
    for idx in out_idx.iter_mut() {
        let old = *idx as usize;
        if used[old] == u32::MAX {
            used[old] = nv.len() as u32;
            nv.push(positions[old]);
        }
        *idx = used[old];
    }
    Some((nv, out_idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regular grid plane: n x n quads.
    fn grid_plane(n: usize) -> (Vec<[f32; 3]>, Vec<u32>) {
        let mut positions = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                positions.push([x as f32, y as f32, 0.0]);
            }
        }
        let at = |x: usize, y: usize| (y * (n + 1) + x) as u32;
        let mut indices = Vec::new();
        for y in 0..n {
            for x in 0..n {
                indices.extend_from_slice(&[at(x, y), at(x + 1, y), at(x + 1, y + 1)]);
                indices.extend_from_slice(&[at(x, y), at(x + 1, y + 1), at(x, y + 1)]);
            }
        }
        (positions, indices)
    }

    #[test]
    fn weld_merges_duplicates() {
        let mut positions = vec![
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0 + 1e-6, 0.0, 0.0], // dup of 1
            [1.0, 1.0, 0.0],
        ];
        let mut indices = vec![0, 1, 2, 3, 4, 2];
        let welded = weld_vertices(&mut positions, &mut indices, 1.0 / 8192.0);
        assert_eq!(welded, 1);
        assert_eq!(positions.len(), 4);
        assert_eq!(indices[3], 1); // remapped to the kept twin
    }

    #[test]
    fn weld_drops_only_doublets_that_have_a_real_third_neighbor() {
        // Group A: triangle (0,1,2) plus an opposite-wound exact duplicate
        // (0,2,1), each of whose 3 edges also has one genuine neighbor
        // triangle (N1/N2/N3). Safe to collapse to a single triangle.
        //
        // Group B: triangle (6,7,8) plus an opposite-wound exact duplicate
        // (6,8,7), completely isolated (no other triangle touches 6/7/8).
        // Collapsing this one would strand all 3 of its edges at boundary
        // count 1, so both copies must survive (matches shipped behavior:
        // a welded hairline-crack doublet, not a provably-redundant one).
        let mut positions = vec![
            [0.0, 0.0, 0.0],   // 0
            [1.0, 0.0, 0.0],   // 1
            [0.0, 1.0, 0.0],   // 2
            [1.0, 1.0, 0.0],   // 3
            [-1.0, 1.0, 0.0],  // 4
            [1.0, -1.0, 0.0],  // 5
            [10.0, 0.0, 0.0],  // 6
            [11.0, 0.0, 0.0],  // 7
            [10.0, 1.0, 0.0],  // 8
        ];
        let mut indices = vec![
            0, 1, 2, // T (kept)
            0, 2, 1, // T doublet (dropped: every edge has a real neighbor)
            1, 0, 3, // N1: shares edge (0,1)
            2, 1, 4, // N2: shares edge (1,2)
            0, 2, 5, // N3: shares edge (0,2)
            6, 7, 8, // U (isolated doublet member, kept)
            6, 8, 7, // U doublet (kept: no real neighbor on any edge)
        ];
        // N1/N2/N3 each contribute 2 unshared outer edges (boundary) plus 1
        // edge shared with group A; group A's edges start at count 3
        // (T + doublet + the one real neighbor) and group B's at count 2
        // (U + doublet only) -- neither is boundary yet, so the baseline is
        // exactly the 3 neighbors' 6 outer edges.
        let before = audit_mesh_topology(&positions, &indices);
        assert_eq!(before.boundary_edges, 6, "unexpected test fixture topology");

        weld_vertices(&mut positions, &mut indices, 1e-6);

        // Only the provably-redundant doublet (Group A) was dropped: 7
        // triangles in, 1 dropped, 6 remain.
        assert_eq!(indices.len() / 3, 6, "expected exactly one dropped triangle");

        // Collapsing Group A must not strand any of its edges as new
        // boundary edges, and Group B's doublet must still fully cover its
        // own edges (unsafe to touch) -- so overall boundary count cannot
        // have increased from the pre-weld baseline.
        let after = audit_mesh_topology(&positions, &indices);
        assert_eq!(
            after.boundary_edges, before.boundary_edges,
            "safe dedupe must never create a new boundary edge"
        );

        // Group B's isolated doublet (6,7,8) is still present twice.
        let group_b_count = indices
            .chunks_exact(3)
            .filter(|tri| {
                let mut v = [tri[0], tri[1], tri[2]];
                v.sort_unstable();
                v == [6, 7, 8]
            })
            .count();
        assert_eq!(group_b_count, 2, "isolated doublet must be preserved");

        // Group A's triangle (0,1,2) now appears exactly once.
        let group_a_count = indices
            .chunks_exact(3)
            .filter(|tri| {
                let mut v = [tri[0], tri[1], tri[2]];
                v.sort_unstable();
                v == [0, 1, 2]
            })
            .count();
        assert_eq!(group_a_count, 1, "redundant doublet must be dropped");
    }

    #[test]
    fn hole_fill_closes_a_missing_face() {
        // Plane with one INTERIOR quad's two triangles removed -> a 4-edge
        // boundary loop (plus the outer boundary, too large at max_loop 8).
        let (positions, mut indices) = grid_plane(4);
        let quad = (1 * 4 + 1) * 6; // quad at (x=1, y=1), interior
        indices.drain(quad..quad + 6);
        let before = indices.len();
        let filled = fill_small_holes(&mut indices, 8);
        assert_eq!(filled, 1);
        assert!(indices.len() > before);
        let _ = positions;
    }

    #[test]
    fn components_drop_floaters() {
        let (mut positions, mut indices) = grid_plane(6);
        // Add a lone floater triangle far away.
        let base = positions.len() as u32;
        positions.push([100.0, 100.0, 0.0]);
        positions.push([101.0, 100.0, 0.0]);
        positions.push([100.0, 101.0, 0.0]);
        indices.extend_from_slice(&[base, base + 1, base + 2]);
        // frac must clear the integer threshold like the crib:
        // thresh = (frac * 72) as int must exceed the floater's 1 face.
        let dropped = drop_small_components(&mut positions, &mut indices, 0.05);
        assert_eq!(dropped, 1);
        assert_eq!(positions.len(), 49);
    }

    #[test]
    fn qem_reaches_target_and_keeps_shape() {
        let (positions, indices) = grid_plane(20); // 800 faces, flat
        let (nv, ni) = decimate_qem(&positions, &indices, 80);
        assert!(ni.len() / 3 <= 100, "faces {}", ni.len() / 3);
        assert!(ni.len() >= 6);
        // Every output vertex stays on the plane and inside the bbox.
        for p in &nv {
            assert!(p[2].abs() < 1e-4);
            assert!(p[0] >= -0.01 && p[0] <= 20.01);
            assert!(p[1] >= -0.01 && p[1] <= 20.01);
        }
        // Total area is preserved (flat plane): sum of face areas ~ 400.
        let mut area = 0.0f64;
        for tri in ni.chunks_exact(3) {
            let a = nv[tri[0] as usize];
            let b = nv[tri[1] as usize];
            let c = nv[tri[2] as usize];
            let u = sub3(b, a);
            let v = sub3(c, a);
            let cr = cross3(u, v);
            area += 0.5 * (dot3(cr, cr) as f64).sqrt();
        }
        // Boundary collapses nibble the border slightly (the crib's boundary
        // weighting allows boundary-to-boundary midpoints): allow 2.5%.
        assert!((area - 400.0).abs() < 10.0, "area {area}");
    }

    #[test]
    fn winding_audit_and_unify_closed_cube() {
        let positions = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let mut indices = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4,
            1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7, 6, 3, 0, 4, 3, 4, 7,
        ];
        // Reverse one face and the entire component: only the local
        // inconsistency is repaired; global winding is intentionally kept.
        indices.swap(1, 2);
        for face in 0..indices.len() / 3 {
            indices.swap(face * 3 + 1, face * 3 + 2);
        }
        let before = audit_mesh_topology(&positions, &indices);
        assert!(before.inconsistent_edges > 0);
        let changed = unify_face_orientations(&positions, &mut indices);
        assert!(changed > 0);
        let after = audit_mesh_topology(&positions, &indices);
        assert_eq!(after.boundary_edges, 0);
        assert_eq!(after.nonmanifold_edges, 0);
        assert_eq!(after.inconsistent_edges, 0);
        assert!(after.signed_volume.abs() > 1.0, "volume {}", after.signed_volume);
    }
}
