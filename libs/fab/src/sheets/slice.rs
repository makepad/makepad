//! Lane E: the plan **geometry core** — slicing, arrangement, loops, fill.
//!
//! Pure data and integer arithmetic: no `Cx`, no widgets, no `fab_scene`
//! beyond `Scene`/`ElementId`, so this module can move into
//! `libs/fab_scene` unchanged the day lane A wants it (report R9).
//!
//! # Why it is written this way
//!
//! Slicing a triangle mesh at a height gives **segment soup**. Turning soup
//! into the closed loops a plan needs (poché, room areas, a clean outline) is
//! the hard part, and floating-point welds die at millimetre scale: two walls
//! that meet exactly in the model land 1e-7 apart after the slice, and an
//! absolute epsilon either misses the join or merges two different corners.
//!
//! So every coordinate is **snap-rounded to a fixed grid** on the way in
//! ([`GRID_PER_METER`] = 0.1 mm) and everything downstream is exact integer
//! arithmetic: equality is equality, a T-junction is a point that lies on a
//! segment, and a loop closes when it returns to the same integer point.
//!
//! The other advantage we exploit: we slice **per element, with its class
//! known** — never anonymous soup. A wall's loops are a wall's loops; a door
//! is not drawn at all but replaced by its symbol; a zone becomes a room.
//!
//! # The pipeline
//!
//! ```text
//! triangles ──slice(plane)──► oriented segments (material on the right)
//!           ──snap_round────► integer segments, degenerate dropped
//!           ──split_t()─────► T-junctions resolved
//!           ──chains()──────► closed loops + open chains, collinear merged
//!           ──area()────────► m², ──spans()──► poché rectangles
//! ```

use crate::api::*;
use makepad_widgets::*;

/// Grid steps per metre: 0.1 mm. A 60 m villa is 600 000 steps across, so
/// every coordinate and every cross product fits an `i64` with room to spare.
pub const GRID_PER_METER: f64 = 10_000.0;

/// A snap-rounded 2D point in grid units.
pub type P2 = [i64; 2];

/// Snap a metre value onto the grid.
#[inline]
pub fn q(v: f32) -> i64 {
    (v as f64 * GRID_PER_METER).round() as i64
}

/// Back to metres.
#[inline]
pub fn unq(v: i64) -> f32 {
    (v as f64 / GRID_PER_METER) as f32
}

pub fn unq2(p: P2) -> [f32; 2] {
    [unq(p[0]), unq(p[1])]
}

/// One oriented segment of a cut. Material is on the **right** of `a → b`,
/// which is what makes the loops consistently wound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    pub a: P2,
    pub b: P2,
    pub element: ElementId,
}

/// A chain of points. `closed` chains are loops and can be filled; open ones
/// are drawn as polylines (a mesh that was not a closed solid).
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub pts: Vec<P2>,
    pub closed: bool,
    pub element: ElementId,
}

impl Chain {
    /// Signed area in square metres (positive = counter-clockwise).
    pub fn signed_area(&self) -> f64 {
        if !self.closed || self.pts.len() < 3 {
            return 0.0;
        }
        let mut acc: i128 = 0;
        for i in 0..self.pts.len() {
            let p = self.pts[i];
            let r = self.pts[(i + 1) % self.pts.len()];
            acc += p[0] as i128 * r[1] as i128 - r[0] as i128 * p[1] as i128;
        }
        (acc as f64) * 0.5 / (GRID_PER_METER * GRID_PER_METER)
    }

    pub fn area(&self) -> f64 {
        self.signed_area().abs()
    }

    /// Axis-aligned bounds in grid units.
    pub fn bounds(&self) -> (P2, P2) {
        let mut lo = [i64::MAX, i64::MAX];
        let mut hi = [i64::MIN, i64::MIN];
        for p in &self.pts {
            lo[0] = lo[0].min(p[0]);
            lo[1] = lo[1].min(p[1]);
            hi[0] = hi[0].max(p[0]);
            hi[1] = hi[1].max(p[1]);
        }
        (lo, hi)
    }

    /// Area-weighted centroid, grid units. Falls back to the bounds centre for
    /// degenerate loops.
    pub fn centroid(&self) -> P2 {
        let a = self.signed_area();
        if !self.closed || a.abs() < 1e-9 {
            let (lo, hi) = self.bounds();
            return [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2];
        }
        let mut cx: i128 = 0;
        let mut cy: i128 = 0;
        for i in 0..self.pts.len() {
            let p = self.pts[i];
            let r = self.pts[(i + 1) % self.pts.len()];
            let cross = p[0] as i128 * r[1] as i128 - r[0] as i128 * p[1] as i128;
            cx += (p[0] as i128 + r[0] as i128) * cross;
            cy += (p[1] as i128 + r[1] as i128) * cross;
        }
        let six_area = 6.0 * a * GRID_PER_METER * GRID_PER_METER;
        [
            (cx as f64 / six_area) as i64,
            (cy as f64 / six_area) as i64,
        ]
    }
}

// ===========================================================================
// Slicing
// ===========================================================================

/// The plane a plan is cut with: horizontal, at `z` metres.
#[derive(Clone, Copy, Debug)]
pub struct CutPlane {
    pub z: f32,
}

/// Slice one triangle. Emits at most one segment, oriented so that the solid
/// side of the triangle's surface is on the right of `a → b`.
fn slice_triangle(tri: [Vec3f; 3], z: f32, element: ElementId, out: &mut Vec<Seg>) {
    let d = [tri[0].z - z, tri[1].z - z, tri[2].z - z];
    // Wholly on one side (touching counts as not crossing: a triangle that
    // only grazes the plane would make a zero-length segment anyway).
    if (d[0] > 0.0 && d[1] > 0.0 && d[2] > 0.0) || (d[0] < 0.0 && d[1] < 0.0 && d[2] < 0.0) {
        return;
    }
    if d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0 {
        return; // coplanar: its edges belong to the neighbouring triangles
    }
    let mut hits: Vec<[f32; 2]> = Vec::with_capacity(2);
    for i in 0..3 {
        let (p, r) = (tri[i], tri[(i + 1) % 3]);
        let (dp, dr) = (d[i], d[(i + 1) % 3]);
        if dp == 0.0 {
            hits.push([p.x, p.y]);
            continue;
        }
        if (dp < 0.0) != (dr < 0.0) && dr != 0.0 {
            let t = dp / (dp - dr);
            hits.push([p.x + (r.x - p.x) * t, p.y + (r.y - p.y) * t]);
        }
    }
    if hits.len() < 2 {
        return;
    }
    let a = [q(hits[0][0]), q(hits[0][1])];
    let b = [q(hits[1][0]), q(hits[1][1])];
    if a == b {
        return;
    }
    // Orientation: the surface normal crossed with +Z points along the cut
    // with the material consistently on one side. Flipping is not an option —
    // every loop in the drawing has to wind the same way.
    let n = Vec3f::cross(tri[1] - tri[0], tri[2] - tri[0]);
    let dir = vec3(n.y, -n.x, 0.0); // cross(n, +Z)
    let seg = [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64];
    let along = dir.x as f64 * seg[0] + dir.y as f64 * seg[1];
    if along >= 0.0 {
        out.push(Seg { a, b, element });
    } else {
        out.push(Seg { a: b, b: a, element });
    }
}

/// Slice a triangle stream. `element` tags every segment so the drawing can
/// keep asking "what is this?".
pub fn slice_triangles(
    tris: impl IntoIterator<Item = [Vec3f; 3]>,
    z: f32,
    element: ElementId,
    out: &mut Vec<Seg>,
) {
    for t in tris {
        slice_triangle(t, z, element, out);
    }
}

/// Every world triangle of one element.
pub fn element_triangles(scene: &Scene, id: ElementId) -> Vec<[Vec3f; 3]> {
    let mut out = Vec::new();
    let Some(el) = scene.element(id) else {
        return out;
    };
    for (bi, first, count) in el.ranges.iter().copied() {
        let Some(batch) = scene.batches.get(bi as usize) else {
            continue;
        };
        let start = first as usize;
        let end = (start + count as usize).min(batch.indices.len());
        let mut i = start;
        while i + 2 < end {
            out.push([
                batch.position(batch.indices[i]),
                batch.position(batch.indices[i + 1]),
                batch.position(batch.indices[i + 2]),
            ]);
            i += 3;
        }
    }
    out
}

/// Slice one element of the scene at height `z`.
pub fn slice_element(scene: &Scene, id: ElementId, z: f32, out: &mut Vec<Seg>) {
    slice_triangles(element_triangles(scene, id), z, id, out);
}

/// The top-down **silhouette** of an element: the edges where an upward-facing
/// triangle meets a downward-facing one, plus mesh boundary edges. This is the
/// outline of the thing as seen from above — what "seen below the cut plane"
/// has to draw, and far cheaper than a projected boolean.
pub fn silhouette(scene: &Scene, id: ElementId, out: &mut Vec<Seg>) {
    use std::collections::HashMap;
    let tris = element_triangles(scene, id);
    // Keyed by the **3D** edge: keying by the projected one merges the top
    // face's diagonal with the bottom face's and turns an interior edge into
    // a false silhouette.
    type E3 = ([i64; 3], [i64; 3]);
    let mut edges: HashMap<E3, (u32, i8)> = HashMap::with_capacity(tris.len() * 3);
    for t in &tris {
        let n = Vec3f::cross(t[1] - t[0], t[2] - t[0]);
        if n.z.abs() < 1e-9 {
            continue; // vertical face: contributes no top-down area
        }
        let sign: i8 = if n.z > 0.0 { 1 } else { -1 };
        for i in 0..3 {
            let p = [q(t[i].x), q(t[i].y), q(t[i].z)];
            let r = [
                q(t[(i + 1) % 3].x),
                q(t[(i + 1) % 3].y),
                q(t[(i + 1) % 3].z),
            ];
            if p == r {
                continue;
            }
            let key = if p < r { (p, r) } else { (r, p) };
            let e = edges.entry(key).or_insert((0, sign));
            e.0 += 1;
            if e.1 != sign {
                e.1 = 0; // up meets down: a silhouette edge
            }
        }
    }
    for ((a, b), (count, sign)) in edges {
        if count == 1 || sign == 0 {
            let (pa, pb) = ([a[0], a[1]], [b[0], b[1]]);
            if pa != pb {
                out.push(Seg { a: pa, b: pb, element: id });
            }
        }
    }
}

// ===========================================================================
// Arrangement
// ===========================================================================

/// Drop degenerate and duplicate segments. Exact, because everything is on
/// the grid by now.
pub fn dedupe(segs: &mut Vec<Seg>) {
    use std::collections::HashSet;
    let mut seen: HashSet<(P2, P2)> = HashSet::with_capacity(segs.len());
    segs.retain(|s| s.a != s.b && seen.insert((s.a, s.b)));
}

/// Is `p` strictly inside the segment `a..b` (and exactly on it)?
fn on_segment(a: P2, b: P2, p: P2) -> bool {
    if p == a || p == b {
        return false;
    }
    let cross = (b[0] - a[0]) as i128 * (p[1] - a[1]) as i128
        - (b[1] - a[1]) as i128 * (p[0] - a[0]) as i128;
    if cross != 0 {
        return false;
    }
    let dot = (p[0] - a[0]) as i128 * (b[0] - a[0]) as i128
        + (p[1] - a[1]) as i128 * (b[1] - a[1]) as i128;
    if dot <= 0 {
        return false;
    }
    let len2 = (b[0] - a[0]) as i128 * (b[0] - a[0]) as i128
        + (b[1] - a[1]) as i128 * (b[1] - a[1]) as i128;
    dot < len2
}

/// Split every segment that another segment's endpoint lands on. Without this
/// a wall that ends against the middle of another wall leaves a loop that
/// walks straight past the junction and never closes.
pub fn split_t_junctions(segs: &mut Vec<Seg>) {
    use std::collections::{HashMap, HashSet};
    if segs.is_empty() {
        return;
    }
    // Bucket the endpoints so each segment only tests points near it.
    let cell = 20_000i64; // 2 m
    let mut grid: HashMap<(i64, i64), Vec<P2>> = HashMap::new();
    let mut points: HashSet<P2> = HashSet::new();
    for s in segs.iter() {
        points.insert(s.a);
        points.insert(s.b);
    }
    for p in &points {
        grid.entry((p[0].div_euclid(cell), p[1].div_euclid(cell)))
            .or_default()
            .push(*p);
    }
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for s in segs.iter() {
        let (x0, x1) = (s.a[0].min(s.b[0]), s.a[0].max(s.b[0]));
        let (y0, y1) = (s.a[1].min(s.b[1]), s.a[1].max(s.b[1]));
        let mut hits: Vec<P2> = Vec::new();
        for gx in x0.div_euclid(cell)..=x1.div_euclid(cell) {
            for gy in y0.div_euclid(cell)..=y1.div_euclid(cell) {
                if let Some(ps) = grid.get(&(gx, gy)) {
                    for p in ps {
                        if on_segment(s.a, s.b, *p) {
                            hits.push(*p);
                        }
                    }
                }
            }
        }
        if hits.is_empty() {
            out.push(*s);
            continue;
        }
        // Order the split points along the segment and re-emit the pieces.
        hits.sort_by_key(|p| {
            (p[0] - s.a[0]).pow(2).saturating_add((p[1] - s.a[1]).pow(2))
        });
        hits.dedup();
        let mut prev = s.a;
        for p in hits {
            if p != prev {
                out.push(Seg { a: prev, b: p, element: s.element });
            }
            prev = p;
        }
        if prev != s.b {
            out.push(Seg { a: prev, b: s.b, element: s.element });
        }
    }
    *segs = out;
}

fn angle_of(a: P2, b: P2) -> f64 {
    ((b[1] - a[1]) as f64).atan2((b[0] - a[0]) as f64)
}

/// Arrange, then walk into chains. Closed chains wind consistently (the
/// slice orientation decides which way); open chains are what is left when the
/// mesh was not a closed solid.
pub fn chains(segs: Vec<Seg>) -> Vec<Chain> {
    walk_chains(arrange(segs))
}

/// Walk an already-arranged segment set into chains. Closed chains wind
/// consistently (the slice orientation decides which way); open chains are
/// what is left when the mesh was not a closed solid.
///
/// At a junction the walk takes the **tightest left turn**, which follows a
/// material-on-the-right (CCW) face of the planar subdivision.
fn walk_chains(segs: Vec<Seg>) -> Vec<Chain> {
    use std::collections::HashMap;
    if segs.is_empty() {
        return Vec::new();
    }
    // point → indices of segments leaving it
    let mut out_of: HashMap<P2, Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        out_of.entry(s.a).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut result = Vec::new();

    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        let element = segs[start].element;
        let mut pts = vec![segs[start].a];
        let mut cur = start;
        used[cur] = true;
        let mut closed = false;
        loop {
            let s = segs[cur];
            pts.push(s.b);
            if s.b == pts[0] {
                pts.pop();
                closed = true;
                break;
            }
            let Some(cands) = out_of.get(&s.b) else {
                break;
            };
            let back = angle_of(s.b, s.a);
            let mut best: Option<(f64, usize)> = None;
            for &c in cands {
                if used[c] {
                    continue;
                }
                // Turn clockwise from the way we came: the largest angle
                // strictly below `back`, wrapping around.
                let a = angle_of(segs[c].a, segs[c].b);
                let mut delta = back - a;
                while delta <= 0.0 {
                    delta += std::f64::consts::TAU;
                }
                while delta > std::f64::consts::TAU {
                    delta -= std::f64::consts::TAU;
                }
                if best.map_or(true, |b| delta < b.0) {
                    best = Some((delta, c));
                }
            }
            match best {
                Some((_, next)) => {
                    used[next] = true;
                    cur = next;
                }
                None => break,
            }
            if pts.len() > 200_000 {
                break;
            }
        }
        if pts.len() >= 2 {
            let mut chain = Chain { pts, closed, element };
            merge_collinear(&mut chain);
            result.push(chain);
        }
    }
    result
}

/// Walk **undirected** segments into chains: every segment is offered in both
/// directions and only the counter-clockwise loops are kept. Silhouettes come
/// out of the mesh without a meaningful direction, so this is how they close.
pub fn outline(segs: Vec<Seg>) -> Vec<Chain> {
    let mut both = Vec::with_capacity(segs.len() * 2);
    for s in segs {
        both.push(s);
        both.push(Seg { a: s.b, b: s.a, element: s.element });
    }
    let mut out = chains(both);
    // Each loop appears twice, once each way; keep the counter-clockwise one.
    out.retain(|c| !c.closed || c.signed_area() > 0.0);
    out
}

// ===========================================================================
// Union — the part that makes overlapping walls into one outline
// ===========================================================================

/// One element's own cut, already closed into loops. Slicing per element and
/// keeping the classification is the whole advantage we have over anonymous
/// segment soup, so the union works on these, not on raw segments.
#[derive(Clone, Debug)]
pub struct Part {
    pub element: ElementId,
    pub loops: Vec<Chain>,
}

/// Even-odd containment in floating point, for the union's inside test.
/// The grid is 0.1 mm over tens of metres, so every coordinate is exact in an
/// `f64` and this is as reliable as the integer version.
fn contains_f(chains: &[Chain], p: [f64; 2]) -> bool {
    let mut inside = false;
    for c in chains.iter().filter(|c| c.closed && c.pts.len() >= 3) {
        let n = c.pts.len();
        for i in 0..n {
            let a = c.pts[i];
            let b = c.pts[(i + 1) % n];
            let (ay, by) = (a[1] as f64, b[1] as f64);
            if (ay > p[1]) != (by > p[1]) {
                let t = (p[1] - ay) / (by - ay);
                let x = a[0] as f64 + (b[0] - a[0]) as f64 * t;
                if x > p[0] {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

/// Proper crossing of two segments (shared endpoints do not count). The
/// intersection is snap-rounded onto the grid like everything else.
fn crossing(a0: P2, a1: P2, b0: P2, b1: P2) -> Option<P2> {
    let r = [(a1[0] - a0[0]) as i128, (a1[1] - a0[1]) as i128];
    let s = [(b1[0] - b0[0]) as i128, (b1[1] - b0[1]) as i128];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom == 0 {
        return None; // parallel or collinear: endpoint splitting handles it
    }
    let qp = [(b0[0] - a0[0]) as i128, (b0[1] - a0[1]) as i128];
    let t_num = qp[0] * s[1] - qp[1] * s[0];
    let u_num = qp[0] * r[1] - qp[1] * r[0];
    let (t_num, u_num, denom) = if denom < 0 {
        (-t_num, -u_num, -denom)
    } else {
        (t_num, u_num, denom)
    };
    if t_num <= 0 || t_num >= denom || u_num <= 0 || u_num >= denom {
        return None;
    }
    let t = t_num as f64 / denom as f64;
    // Snap-round onto the grid. The rounded point may land on an endpoint of
    // one segment (a near-miss T) — still return it so the *other* segment
    // is split. Dropping the pair here was the self-intersection bug: the
    // second edge kept walking through the first.
    Some([
        a0[0] + (r[0] as f64 * t).round() as i64,
        a0[1] + (r[1] as f64 * t).round() as i64,
    ])
}

/// Projection of `p` onto `a → b`, used to order split vertices along an edge.
fn along(a: P2, b: P2, p: P2) -> i128 {
    (p[0] - a[0]) as i128 * (b[0] - a[0]) as i128
        + (p[1] - a[1]) as i128 * (b[1] - a[1]) as i128
}

/// Emit each segment split at the given vertices (already snap-rounded).
fn emit_pieces(segs: &[Seg], cuts: &[Vec<P2>]) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        if cuts[i].is_empty() {
            if s.a != s.b {
                out.push(*s);
            }
            continue;
        }
        let mut ps = cuts[i].clone();
        ps.sort_by_key(|p| along(s.a, s.b, *p));
        ps.dedup();
        let mut prev = s.a;
        for p in ps {
            if p != prev && p != s.b {
                out.push(Seg {
                    a: prev,
                    b: p,
                    element: s.element,
                });
                prev = p;
            }
        }
        if prev != s.b {
            out.push(Seg {
                a: prev,
                b: s.b,
                element: s.element,
            });
        }
    }
    out
}

/// Proper crossings via an x-sweep: segments are inserted when the sweep
/// reaches their left endpoint and tested against the currently active set
/// (those whose x-range covers the sweep). Architectural plans are sparse
/// in x so the active set stays small — O((n + I) log n) typical.
pub fn find_crossings(segs: &[Seg]) -> Vec<(usize, usize, P2)> {
    let n = segs.len();
    if n < 2 {
        return Vec::new();
    }
    let mut events: Vec<(i64, u8, i64, usize)> = Vec::with_capacity(n * 2);
    for (i, s) in segs.iter().enumerate() {
        let x0 = s.a[0].min(s.b[0]);
        let x1 = s.a[0].max(s.b[0]);
        let y0 = s.a[1].min(s.b[1]);
        events.push((x0, 0, y0, i)); // start
        events.push((x1, 1, y0, i)); // end
    }
    events.sort_unstable();
    let mut active: Vec<usize> = Vec::new();
    let mut hits = Vec::new();
    for (_x, kind, _y, i) in events {
        if kind == 0 {
            let (yi0, yi1) = (
                segs[i].a[1].min(segs[i].b[1]),
                segs[i].a[1].max(segs[i].b[1]),
            );
            for &j in &active {
                let (yj0, yj1) = (
                    segs[j].a[1].min(segs[j].b[1]),
                    segs[j].a[1].max(segs[j].b[1]),
                );
                if yi1 < yj0 || yj1 < yi0 {
                    continue;
                }
                if let Some(p) = crossing(segs[i].a, segs[i].b, segs[j].a, segs[j].b) {
                    let on_i = p == segs[i].a || p == segs[i].b;
                    let on_j = p == segs[j].a || p == segs[j].b;
                    if on_i && on_j {
                        continue;
                    }
                    hits.push((i.min(j), i.max(j), p));
                }
            }
            active.push(i);
        } else if let Some(pos) = active.iter().position(|&k| k == i) {
            active.swap_remove(pos);
        }
    }
    hits
}

/// Split every segment where another segment crosses it.
fn split_crossings(segs: &mut Vec<Seg>) {
    let hits = find_crossings(segs);
    if hits.is_empty() {
        return;
    }
    let mut cuts: Vec<Vec<P2>> = vec![Vec::new(); segs.len()];
    for (i, j, p) in hits {
        cuts[i].push(p);
        cuts[j].push(p);
    }
    *segs = emit_pieces(segs, &cuts);
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Canonical integer line through `a` and `b`: reduced direction + offset.
fn line_key(a: P2, b: P2) -> Option<(i64, i64, i128)> {
    let mut dx = b[0] - a[0];
    let mut dy = b[1] - a[1];
    if dx == 0 && dy == 0 {
        return None;
    }
    let g = gcd_u64(dx.unsigned_abs(), dy.unsigned_abs()).max(1) as i64;
    dx /= g;
    dy /= g;
    if dx < 0 || (dx == 0 && dy < 0) {
        dx = -dx;
        dy = -dy;
    }
    let c = dy as i128 * a[0] as i128 - dx as i128 * a[1] as i128;
    Some((dx, dy, c))
}

/// Split collinear overlapping runs at every shared vertex so identical
/// atomic pieces can be deduped / cancelled. Without this two walls that
/// share a stretch of face leave doubled edges that the walk treats as a
/// corridor.
fn merge_collinear_overlaps(segs: &mut Vec<Seg>) {
    use std::collections::HashMap;
    let mut groups: HashMap<(i64, i64, i128), Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        if let Some(k) = line_key(s.a, s.b) {
            groups.entry(k).or_default().push(i);
        }
    }
    let mut cuts: Vec<Vec<P2>> = vec![Vec::new(); segs.len()];
    let mut any = false;
    for idxs in groups.values() {
        if idxs.len() < 2 {
            continue;
        }
        let mut pts: Vec<P2> = Vec::with_capacity(idxs.len() * 2);
        for &i in idxs {
            pts.push(segs[i].a);
            pts.push(segs[i].b);
        }
        pts.sort_unstable();
        pts.dedup();
        for &i in idxs {
            for &p in &pts {
                if on_segment(segs[i].a, segs[i].b, p) {
                    cuts[i].push(p);
                    any = true;
                }
            }
        }
    }
    if any {
        *segs = emit_pieces(segs, &cuts);
    }
}

/// Snap-round arrangement: split at every proper crossing and every
/// T-junction, iterate until the graph is planar, then merge collinear
/// overlaps. Coordinates stay on the integer grid the whole way — there is
/// no float-epsilon weld.
pub fn arrange(mut segs: Vec<Seg>) -> Vec<Seg> {
    dedupe(&mut segs);
    for _ in 0..8 {
        let before = segs.len();
        split_crossings(&mut segs);
        split_t_junctions(&mut segs);
        merge_collinear_overlaps(&mut segs);
        dedupe(&mut segs);
        if segs.len() == before && find_crossings(&segs).is_empty() {
            break;
        }
    }
    segs.retain(|s| s.a != s.b);
    segs
}

/// Number of proper crossings left in the chains. Zero after [`arrange`].
pub fn chain_crossings(chains: &[Chain]) -> usize {
    let mut segs = Vec::new();
    for c in chains {
        let n = c.pts.len();
        if n < 2 {
            continue;
        }
        let last = if c.closed { n } else { n - 1 };
        for i in 0..last {
            segs.push(Seg {
                a: c.pts[i],
                b: c.pts[(i + 1) % n],
                element: c.element,
            });
        }
    }
    find_crossings(&segs).len()
}

/// Where two solids touch face to face the same edge arrives twice, once in
/// each direction: it is interior to the union and its midpoint is on both
/// boundaries, so the inside test cannot see it. Cancel such pairs.
fn drop_shared_faces(segs: &mut Vec<Seg>) {
    use std::collections::HashSet;
    let present: HashSet<(P2, P2)> = segs.iter().map(|s| (s.a, s.b)).collect();
    segs.retain(|s| !present.contains(&(s.b, s.a)));
}

/// Union the parts into one set of loops.
///
/// Walls that meet at a corner **overlap** in the model — the cut of each is a
/// correct rectangle, and where two of them overlap the edges run through
/// solid material. Drawing those is what makes a generated plan look like a
/// pile of boxes instead of a building. So: split everything at every
/// crossing, throw away the pieces that lie inside another part, and walk what
/// is left. The result is the outline of the union plus the rooms inside it.
pub fn union_parts(parts: &[Part]) -> Vec<Chain> {
    let mut segs: Vec<Seg> = Vec::new();
    for p in parts {
        for c in &p.loops {
            let n = c.pts.len();
            if n < 2 {
                continue;
            }
            let last = if c.closed { n } else { n - 1 };
            for i in 0..last {
                segs.push(Seg {
                    a: c.pts[i],
                    b: c.pts[(i + 1) % n],
                    element: p.element,
                });
            }
        }
    }
    segs = arrange(segs);
    drop_shared_faces(&mut segs);

    // Drop the pieces buried inside some *other* part.
    //
    // The test is not "is the edge inside something" — an edge of one wall
    // lies exactly *on* the boundary of the wall it butts into, and a
    // point-in-polygon test on a boundary point answers by coin flip. The
    // question that has an answer is: **is the empty side of this edge filled
    // by someone else?** Material is on the right of `a → b`, so we probe one
    // millimetre to the left. If that is solid, this edge is interior to the
    // union and must not be drawn.
    let eps = 10.0; // grid units = 1 mm, thinner than any real element
    let mut kept: Vec<Seg> = Vec::with_capacity(segs.len());
    for s in &segs {
        let d = [(s.b[0] - s.a[0]) as f64, (s.b[1] - s.a[1]) as f64];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
        let left = [-d[1] / len, d[0] / len];
        let probe = [
            (s.a[0] as f64 + s.b[0] as f64) * 0.5 + left[0] * eps,
            (s.a[1] as f64 + s.b[1] as f64) * 0.5 + left[1] * eps,
        ];
        let buried = parts
            .iter()
            .any(|p| p.element != s.element && contains_f(&p.loops, probe));
        if !buried {
            kept.push(*s);
        }
    }
    walk_chains(kept)
}

/// Cut one element and close it into its own loops.
pub fn part_of(scene: &Scene, id: ElementId, z: f32) -> Part {
    let mut segs = Vec::new();
    slice_element(scene, id, z, &mut segs);
    Part {
        element: id,
        loops: chains(segs),
    }
}

/// Drop vertices that sit exactly on the straight line between their
/// neighbours. Cartographic cleanup: a sliced wall arrives as dozens of
/// collinear pieces (one per triangle) and every one of them is a join the
/// renderer would otherwise draw.
pub fn merge_collinear(chain: &mut Chain) {
    if chain.pts.len() < 3 {
        return;
    }
    let n = chain.pts.len();
    let mut keep = Vec::with_capacity(n);
    for i in 0..n {
        if !chain.closed && (i == 0 || i == n - 1) {
            keep.push(chain.pts[i]);
            continue;
        }
        let p = chain.pts[(i + n - 1) % n];
        let c = chain.pts[i];
        let r = chain.pts[(i + 1) % n];
        let cross = (c[0] - p[0]) as i128 * (r[1] - p[1]) as i128
            - (c[1] - p[1]) as i128 * (r[0] - p[0]) as i128;
        if cross != 0 {
            keep.push(c);
        }
    }
    if keep.len() >= 2 {
        chain.pts = keep;
    }
}

/// Throw away loops too small to draw: at 1:100 a 0.4 mm sliver on paper is
/// 40 mm in the model, and drawing it only makes the line weights lie.
pub fn cull_slivers(chains: &mut Vec<Chain>, min_paper_mm: f32, scale: f32) {
    let min_model_m = (min_paper_mm / 1000.0) * scale;
    let min_grid = q(min_model_m).max(1);
    chains.retain(|c| {
        let (lo, hi) = c.bounds();
        (hi[0] - lo[0]) >= min_grid || (hi[1] - lo[1]) >= min_grid
    });
}

// ===========================================================================
// Poché
// ===========================================================================

/// One filled rectangle of the poché, in grid units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub x0: i64,
    pub x1: i64,
    pub y0: i64,
    pub y1: i64,
}

/// Scan-convert closed loops into merged rectangles (even-odd rule).
///
/// Vertically adjacent scanlines whose spans line up are merged into one
/// rectangle, so a straight wall costs one rectangle rather than one per
/// scanline — the difference between 300 primitives and 30 000.
pub fn spans(chains: &[Chain], step_m: f32) -> Vec<Span> {
    let step = q(step_m).max(1);
    let mut lo = [i64::MAX, i64::MAX];
    let mut hi = [i64::MIN, i64::MIN];
    let mut any = false;
    for c in chains.iter().filter(|c| c.closed && c.pts.len() >= 3) {
        any = true;
        let (l, h) = c.bounds();
        lo[0] = lo[0].min(l[0]);
        lo[1] = lo[1].min(l[1]);
        hi[0] = hi[0].max(h[0]);
        hi[1] = hi[1].max(h[1]);
    }
    if !any {
        return Vec::new();
    }
    // open rectangles, keyed by their span, extended while the next scanline
    // repeats them
    let mut open: Vec<Span> = Vec::new();
    let mut done: Vec<Span> = Vec::new();
    let tol = step; // spans within one step count as "the same wall"

    let mut y = lo[1] + step / 2;
    while y <= hi[1] {
        let mut xs: Vec<i64> = Vec::new();
        for c in chains.iter().filter(|c| c.closed && c.pts.len() >= 3) {
            let n = c.pts.len();
            for i in 0..n {
                let p = c.pts[i];
                let r = c.pts[(i + 1) % n];
                if (p[1] > y) == (r[1] > y) {
                    continue;
                }
                let t = (y - p[1]) as f64 / (r[1] - p[1]) as f64;
                xs.push(p[0] + ((r[0] - p[0]) as f64 * t).round() as i64);
            }
        }
        if xs.len() >= 2 {
            xs.sort_unstable();
            let mut row: Vec<(i64, i64)> = Vec::new();
            let mut i = 0;
            while i + 1 < xs.len() {
                let (a, b) = (xs[i], xs[i + 1]);
                if b > a {
                    match row.last_mut() {
                        Some(last) if a - last.1 <= 1 => last.1 = b,
                        _ => row.push((a, b)),
                    }
                }
                i += 2;
            }
            let mut next_open: Vec<Span> = Vec::with_capacity(row.len());
            let mut matched = vec![false; open.len()];
            for (a, b) in row {
                let mut hit = None;
                for (k, o) in open.iter().enumerate() {
                    if !matched[k] && (o.x0 - a).abs() <= tol && (o.x1 - b).abs() <= tol {
                        hit = Some(k);
                        break;
                    }
                }
                match hit {
                    Some(k) => {
                        matched[k] = true;
                        let mut o = open[k];
                        o.y1 = y + step / 2;
                        o.x0 = o.x0.min(a);
                        o.x1 = o.x1.max(b);
                        next_open.push(o);
                    }
                    None => next_open.push(Span {
                        x0: a,
                        x1: b,
                        y0: y - step / 2,
                        y1: y + step / 2,
                    }),
                }
            }
            for (k, o) in open.iter().enumerate() {
                if !matched[k] {
                    done.push(*o);
                }
            }
            open = next_open;
        } else {
            done.append(&mut open);
        }
        y += step;
    }
    done.append(&mut open);
    done
}

/// Is the point inside the loops (even-odd)? Used to place a room label where
/// the room actually is, not where its bounding box says.
pub fn contains(chains: &[Chain], p: P2) -> bool {
    let mut inside = false;
    for c in chains.iter().filter(|c| c.closed && c.pts.len() >= 3) {
        let n = c.pts.len();
        for i in 0..n {
            let a = c.pts[i];
            let b = c.pts[(i + 1) % n];
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let t = (p[1] - a[1]) as f64 / (b[1] - a[1]) as f64;
                let x = a[0] as f64 + (b[0] - a[0]) as f64 * t;
                if x > p[0] as f64 {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six faces of an axis-aligned box, wound outward.
    fn box_tris(min: [f32; 3], max: [f32; 3]) -> Vec<[Vec3f; 3]> {
        let (a, b) = (min, max);
        let v = |x: f32, y: f32, z: f32| vec3(x, y, z);
        let c = [
            v(a[0], a[1], a[2]),
            v(b[0], a[1], a[2]),
            v(b[0], b[1], a[2]),
            v(a[0], b[1], a[2]),
            v(a[0], a[1], b[2]),
            v(b[0], a[1], b[2]),
            v(b[0], b[1], b[2]),
            v(a[0], b[1], b[2]),
        ];
        let quad = |i: usize, j: usize, k: usize, l: usize| {
            vec![[c[i], c[j], c[k]], [c[i], c[k], c[l]]]
        };
        let mut t = Vec::new();
        t.extend(quad(0, 3, 2, 1)); // bottom (−Z)
        t.extend(quad(4, 5, 6, 7)); // top (+Z)
        t.extend(quad(0, 1, 5, 4)); // −Y
        t.extend(quad(1, 2, 6, 5)); // +X
        t.extend(quad(2, 3, 7, 6)); // +Y
        t.extend(quad(3, 0, 4, 7)); // −X
        t
    }

    /// One element per box, cut and unioned — exactly what a plan does.
    fn slice_all(boxes: &[([f32; 3], [f32; 3])], z: f32) -> Vec<Chain> {
        let parts: Vec<Part> = boxes
            .iter()
            .enumerate()
            .map(|(i, (mn, mx))| {
                let mut segs = Vec::new();
                let id = ElementId::from_index(i);
                slice_triangles(box_tris(*mn, *mx), z, id, &mut segs);
                Part { element: id, loops: chains(segs) }
            })
            .collect();
        union_parts(&parts)
    }

    #[test]
    fn a_box_cuts_to_one_closed_square() {
        let cs = slice_all(&[([0.0, 0.0, 0.0], [4.0, 3.0, 2.5])], 1.2);
        let closed: Vec<&Chain> = cs.iter().filter(|c| c.closed).collect();
        assert_eq!(closed.len(), 1, "{cs:#?}");
        assert_eq!(closed[0].pts.len(), 4, "collinear pieces not merged: {:?}", closed[0].pts);
        assert!((closed[0].area() - 12.0).abs() < 1e-6, "area {}", closed[0].area());
        assert_eq!(chain_crossings(&cs), 0);
    }

    /// Two walls that cross in plan: after snap-rounding the intersection the
    /// graph must be planar — no remaining proper crossings.
    #[test]
    fn crossing_walls_arrange_without_self_intersection() {
        let walls = [
            ([0.0, 1.0, 0.0], [4.0, 1.3, 3.0]),
            ([1.5, 0.0, 0.0], [1.8, 4.0, 3.0]),
        ];
        let cs = slice_all(&walls, 1.2);
        assert_eq!(chain_crossings(&cs), 0, "arrangement left crossings: {cs:#?}");
        let closed: Vec<&Chain> = cs.iter().filter(|c| c.closed).collect();
        assert!(!closed.is_empty(), "expected closed loops, got {cs:#?}");
        let area: f64 = closed.iter().map(|c| c.area()).sum();
        // 4.0×0.3 + 0.3×4.0 − 0.3×0.3 overlap = 2.31
        assert!((area - 2.31).abs() / 2.31 < 0.005, "union area {area}");
    }

    #[test]
    fn two_diagonals_split_at_the_grid_point() {
        let segs = vec![
            Seg {
                a: [q(0.0), q(0.0)],
                b: [q(2.0), q(2.0)],
                element: ElementId::from_index(0),
            },
            Seg {
                a: [q(0.0), q(2.0)],
                b: [q(2.0), q(0.0)],
                element: ElementId::from_index(1),
            },
        ];
        let arranged = arrange(segs);
        assert_eq!(find_crossings(&arranged).len(), 0);
        assert_eq!(arranged.len(), 4, "{arranged:?}");
        let mid = [q(1.0), q(1.0)];
        assert!(arranged.iter().all(|s| s.a == mid || s.b == mid));
    }

    /// Four walls around a room: the cut is a closed outer loop and a closed
    /// inner loop, and the inner one is the room.
    #[test]
    fn four_walls_make_an_outer_and_an_inner_loop() {
        let t = 0.3;
        let (w, d) = (5.0f32, 4.0f32);
        let walls = [
            ([0.0, 0.0, 0.0], [w, t, 3.0]),
            ([0.0, d - t, 0.0], [w, d, 3.0]),
            ([0.0, t, 0.0], [t, d - t, 3.0]),
            ([w - t, t, 0.0], [w, d - t, 3.0]),
        ];
        let cs = slice_all(&walls, 1.2);
        let mut closed: Vec<f64> = cs.iter().filter(|c| c.closed).map(|c| c.area()).collect();
        closed.sort_by(|a, b| b.partial_cmp(a).unwrap());
        assert!(closed.len() >= 2, "expected an outer and an inner loop, got {closed:?}");
        // outer 5 × 4 = 20, inner 4.4 × 3.4 = 14.96
        assert!((closed[0] - 20.0).abs() < 0.02, "outer {}", closed[0]);
        assert!((closed[1] - 14.96).abs() < 0.02, "inner {}", closed[1]);
        assert_eq!(chain_crossings(&cs), 0);
    }

    /// An L-shaped room: the inner loop has six corners and the right area.
    #[test]
    fn an_l_shaped_room_closes_with_six_corners() {
        let t = 0.2;
        // Outer footprint 6 × 5 with a 2 × 2 bite out of the +X +Y corner.
        let walls = [
            ([0.0, 0.0, 0.0], [6.0, t, 3.0]),          // south
            ([0.0, 0.0, 0.0], [t, 5.0, 3.0]),          // west
            ([0.0, 5.0 - t, 0.0], [4.0, 5.0, 3.0]),    // north (short)
            ([4.0 - t, 3.0, 0.0], [4.0, 5.0, 3.0]),    // the bite's west wall
            ([4.0, 3.0 - t, 0.0], [6.0, 3.0, 3.0]),    // the bite's south wall
            ([6.0 - t, 0.0, 0.0], [6.0, 3.0, 3.0]),    // east (short)
        ];
        let cs = slice_all(&walls, 1.5);
        let inner: Vec<&Chain> = cs
            .iter()
            .filter(|c| c.closed && c.area() > 5.0 && c.area() < 25.0)
            .collect();
        assert!(!inner.is_empty(), "no room loop: {:?}", cs.iter().map(|c| (c.closed, c.area())).collect::<Vec<_>>());
        let room = inner.iter().max_by(|a, b| a.area().partial_cmp(&b.area()).unwrap()).unwrap();
        // Inner L hexagon: (t,t)–(6−t,t)–(6−t,3−t)–(4−t,3−t)–(4−t,5−t)–(t,5−t)
        // = (6−2t)×(3−2t) + (4−2t)×2 = 5.6×2.6 + 3.6×2 = 21.76 m²
        let expect = 21.76;
        assert!(
            (room.area() - expect).abs() / expect < 0.005,
            "L area {}, expected {expect} ±0.5%",
            room.area()
        );
        assert!(room.pts.len() >= 6, "L should have ≥ 6 corners, has {}", room.pts.len());
        assert_eq!(chain_crossings(&cs), 0);
    }

    /// A wall that ends against the middle of another wall: without splitting
    /// the T-junction the loop walks past it and never closes.
    #[test]
    fn a_t_junction_splits_and_closes() {
        let t = 0.2;
        let walls = [
            ([0.0, 0.0, 0.0], [6.0, t, 3.0]),
            ([0.0, 4.0, 0.0], [6.0, 4.0 + t, 3.0]),
            ([0.0, 0.0, 0.0], [t, 4.0 + t, 3.0]),
            ([6.0 - t, 0.0, 0.0], [6.0, 4.0 + t, 3.0]),
            // the spur, ending in the middle of the north wall
            ([3.0, t, 0.0], [3.0 + t, 4.0, 3.0]),
        ];
        let cs = slice_all(&walls, 1.2);
        let closed: Vec<&Chain> = cs.iter().filter(|c| c.closed).collect();
        assert!(
            closed.len() >= 3,
            "expected outer + two rooms, got {:?} (open: {:?})",
            closed.iter().map(|c| (c.area(), c.pts.len())).collect::<Vec<_>>(),
            cs.iter().filter(|c| !c.closed).map(|c| c.pts.clone()).collect::<Vec<_>>()
        );
        let mut areas: Vec<f64> = closed.iter().map(|c| c.area()).collect();
        areas.sort_by(|a, b| b.partial_cmp(a).unwrap());
        assert!((areas[0] - 6.0 * 4.2).abs() < 0.05, "outer {}", areas[0]);
        // two rooms of (3 − 0.3) × 3.8 ≈ 10.3 and (2.8 − 0.1) × 3.8
        assert!(areas[1] > 8.0 && areas[1] < 12.0, "room a {}", areas[1]);
        assert!(areas[2] > 8.0 && areas[2] < 12.0, "room b {}", areas[2]);
        assert_eq!(chain_crossings(&cs), 0);
    }

    #[test]
    fn two_storeys_cut_independently() {
        let ground = ([0.0, 0.0, 0.0], [5.0, 4.0, 3.0]);
        let upper = ([1.0, 1.0, 3.0], [4.0, 3.0, 6.0]);
        let low = slice_all(&[ground, upper], 1.2);
        let high = slice_all(&[ground, upper], 4.5);
        let a: f64 = low.iter().filter(|c| c.closed).map(|c| c.area()).sum();
        let b: f64 = high.iter().filter(|c| c.closed).map(|c| c.area()).sum();
        assert!((a - 20.0).abs() < 1e-3, "ground {a}");
        assert!((b - 6.0).abs() < 1e-3, "upper {b}");
        assert_eq!(chain_crossings(&low), 0);
        assert_eq!(chain_crossings(&high), 0);
    }

    #[test]
    fn poche_spans_cover_the_wall_and_merge_vertically() {
        let t = 0.3;
        let walls = [([0.0, 0.0, 0.0], [5.0, t, 3.0])];
        let cs = slice_all(&walls, 1.2);
        let sp = spans(&cs, 0.02);
        assert!(!sp.is_empty());
        // One straight wall must not explode into a rectangle per scanline.
        assert!(sp.len() <= 4, "{} spans for one wall", sp.len());
        let area: f64 = sp
            .iter()
            .map(|s| (s.x1 - s.x0) as f64 * (s.y1 - s.y0) as f64)
            .sum::<f64>()
            / (GRID_PER_METER * GRID_PER_METER);
        assert!((area - 1.5).abs() < 0.1, "poché area {area}, wanted 1.5");
    }

    #[test]
    fn contains_finds_the_inside_of_a_room() {
        let cs = slice_all(&[([0.0, 0.0, 0.0], [4.0, 4.0, 3.0])], 1.0);
        assert!(contains(&cs, [q(2.0), q(2.0)]));
        assert!(!contains(&cs, [q(5.0), q(2.0)]));
    }

    #[test]
    fn silhouette_of_a_box_is_its_footprint() {
        // Build a one-element scene out of the demo house and check the
        // silhouette of a slab closes over its own footprint.
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let slab = scene
            .elements
            .iter()
            .find(|e| e.class == crate::model::ElementClass::Slab && e.has_geometry())
            .expect("a slab");
        let mut segs = Vec::new();
        silhouette(&scene, slab.id, &mut segs);
        assert!(!segs.is_empty());
        let cs = outline(segs);
        let biggest = cs
            .iter()
            .filter(|c| c.closed)
            .max_by(|a, b| a.area().partial_cmp(&b.area()).unwrap());
        let e = aabb_extent(&slab.bounds);
        let expect = (e.x * e.y) as f64;
        let got = biggest.map(|c| c.area()).unwrap_or(0.0);
        assert!(
            (got - expect).abs() < expect * 0.05,
            "silhouette area {got}, footprint {expect}"
        );
    }
}
