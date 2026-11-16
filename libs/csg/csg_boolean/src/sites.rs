use crate::classes::Classes;
use crate::keys::{dominant_axis, Key};
use crate::virtual_points::VirtualPoints;
use makepad_csg_math::Vec3d;
use std::cmp::Ordering;
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};
use std::sync::Arc;

/// A triangle's point ids with aliases applied, and its index in its operand.
#[derive(Clone, Copy)]
pub(crate) struct PairTriangle {
    pub(crate) ids: [u32; 3],
    pub(crate) index: u32,
}

/// What one candidate pair gives the cut.
#[derive(Default)]
pub(crate) struct PairSites {
    /// Segments that become edges of both triangles' pieces.
    pub(crate) segments: Vec<[Key; 2]>,
    /// Input vertices on another operand's edge: operand, ascending edge ids, vertex key.
    pub(crate) on_edges: Vec<(u8, [u32; 2], Key)>,
    /// The pair is coplanar in the virtual set.
    pub(crate) coplanar: bool,
    /// Parts with more than two distinct ends, or missed aliases (cut counter 0).
    pub(crate) broken: usize,
}

/// Each triangle's projection axis and winding (`orient2d_along` of its own corners on that axis), for the
/// pairs that are not coplanar: there both depend on the triangle alone. A slot is filled on first use; its
/// value is a pure function of the triangle, so two threads that race write the same byte.
pub(crate) struct WindingMemo {
    slots: [Vec<AtomicU8>; 2],
}

impl WindingMemo {
    pub(crate) fn new(counts: [usize; 2]) -> WindingMemo {
        WindingMemo { slots: counts.map(|n| (0..n).map(|_| AtomicU8::new(0)).collect()) }
    }

    /// (axis, winding) of triangle `t` of `operand`. 0 = not yet known, else 1 + 3 axis + (winding + 1).
    fn get(&self, points: &VirtualPoints, operand: usize, t: PairTriangle) -> (usize, i32) {
        let slot = &self.slots[operand][t.index as usize];
        let mut value = slot.load(AtomicOrdering::Relaxed);
        if value == 0 {
            let axis = projection_axis(points, t, None);
            let [p, q, r] = t.ids;
            let winding = points.orient2d_along(axis, p, q, r);
            value = 1 + 3 * axis as u8 + (winding + 1) as u8;
            slot.store(value, AtomicOrdering::Relaxed);
        }
        let value = value - 1;
        ((value / 3) as usize, (value % 3) as i32 - 1)
    }
}

/// A triangle's plane in floats, for the six sides of `pair_sites`: built once per corefinement from the
/// operand's own corners. `side` answers only when the float value proves the sign of the exact `orient3d`.
#[derive(Clone, Copy)]
pub(crate) struct TrianglePlane {
    p: Vec3d,
    n: Vec3d,
    /// Per axis k, |e1_i e2_j| + |e1_j e2_i| (i, j the other two axes): the permanent of n's component k.
    m: [f64; 3],
    /// |n|, computed as the scan computes its normal's length; |e1| and |e2|.
    nlen: f64,
    l1: f64,
    l2: f64,
    /// The displacement of p, and the two Hadamard factors of the corners' displacements.
    dp: f64,
    h1: f64,
    h2: f64,
}

impl TrianglePlane {
    /// The geometric half, from the triangle's input corners (p, q, r), before any point is moved. Its
    /// displacement terms are 0 until `displaced`.
    pub(crate) fn geometric((p, q, r): (Vec3d, Vec3d, Vec3d)) -> TrianglePlane {
        let (e1, e2) = (q - p, r - p);
        let n = e1.cross(e2);
        let m = [
            (e1.y * e2.z).abs() + (e1.z * e2.y).abs(),
            (e1.z * e2.x).abs() + (e1.x * e2.z).abs(),
            (e1.x * e2.y).abs() + (e1.y * e2.x).abs(),
        ];
        TrianglePlane { p, n, m, nlen: n.length(), l1: e1.length(), l2: e2.length(), dp: 0.0, h1: 0.0, h2: 0.0 }
    }

    /// The record with its displacement half, for the operand's own corner ids [p, q, r] after the classes
    /// (their input positions are the corners `geometric` was given).
    pub(crate) fn displaced(&self, points: &VirtualPoints, [p, q, r]: [u32; 3]) -> TrianglePlane {
        debug_assert_eq!(self.p, points.real(p), "the record's corner is the point's input position");
        let (dp, dq, dr) = (points.displacement(p), points.displacement(q), points.displacement(r));
        let (u, w) = (self.l1, self.l2);
        let (du, dw) = (dq + dp, dr + dp);
        TrianglePlane { dp, h1: du * w + u * dw + du * dw, h2: u * w + du * w + u * dw + du * dw, ..*self }
    }

    /// The side of v against the plane, as `orient3d(p, q, r, v)` gives it, when the float value proves it.
    /// Rounding of the record and the dot product is about 8 units of the permanent; 1e-14 is 11 times that.
    /// The displacements are filter3d's Hadamard terms regrouped, with |d|_1 >= |d|. NaN fails both tests.
    fn side(&self, points: &VirtualPoints, v: u32) -> Option<i32> {
        let d = points.real(v) - self.p;
        let s = self.n.dot(d);
        let (ax, ay, az) = (d.x.abs(), d.y.abs(), d.z.abs());
        let bound = (1e-14 * (ax * self.m[0] + ay * self.m[1] + az * self.m[2])
            + (ax + ay + az) * self.h1
            + (points.displacement(v) + self.dp) * self.h2)
            * (1.0 + 1e-9);
        if s > bound {
            Some(1)
        } else if s < -bound {
            Some(-1)
        } else {
            None
        }
    }

    /// Whether x lies provably beyond the coplanar scan's band of this plane: the scan's `orient3d` of the
    /// corners and x exceeds `tol` times its normal's length, which is `nlen` (the same float). The record's
    /// value differs from the exact determinant by at most 1e-14 of the permanent; the scan's robust value
    /// from it by 7.8e-16 of a permanent that is at most 6 l3 (l3 + l1)(l3 + l2), l3 = |d|_1.
    pub(crate) fn beyond(&self, x: Vec3d, tol: f64) -> bool {
        let d = x - self.p;
        let s = self.n.dot(d);
        let (ax, ay, az) = (d.x.abs(), d.y.abs(), d.z.abs());
        let l3 = ax + ay + az;
        let margin = (tol * self.nlen
            + 1e-14 * (ax * self.m[0] + ay * self.m[1] + az * self.m[2])
            + 1e-14 * l3 * (l3 + self.l1) * (l3 + self.l2))
            * (1.0 + 1e-9);
        s.abs() > margin
    }
}

/// The plane records of the triangles the candidate pairs name, per operand: `slot[operand][triangle]` is the
/// record's index in `records[operand]` (u32::MAX for a triangle no pair names; it is never read), `order` the
/// triangles in record order. `geometric` builds the geometric half before the classes, `displaced` the full
/// record after; both share the index.
pub(crate) struct PlaneRecords {
    slot: Arc<[Vec<u32>; 2]>,
    order: Arc<[Vec<u32>; 2]>,
    records: [Vec<TrianglePlane>; 2],
}

impl PlaneRecords {
    /// The geometric half for every triangle a pair names, in the order the pairs first name them.
    pub(crate) fn geometric(pairs: &[(u32, u32)], triangles: [&[(Vec3d, Vec3d, Vec3d)]; 2]) -> PlaneRecords {
        let mut slot = [vec![u32::MAX; triangles[0].len()], vec![u32::MAX; triangles[1].len()]];
        let mut order = [Vec::new(), Vec::new()];
        let mut records = [Vec::new(), Vec::new()];
        for &(a, b) in pairs {
            for (operand, triangle) in [(0, a), (1, b)] {
                let entry = &mut slot[operand][triangle as usize];
                if *entry == u32::MAX {
                    *entry = records[operand].len() as u32;
                    records[operand].push(TrianglePlane::geometric(triangles[operand][triangle as usize]));
                    order[operand].push(triangle);
                }
            }
        }
        PlaneRecords { slot: Arc::new(slot), order: Arc::new(order), records }
    }

    /// The full records, for the operands' own corner ids after the classes: `ids[operand][triangle]` are the
    /// triangle's corners in its operand's mesh, and operand B's own ids are its mesh ids plus `offset_b`.
    pub(crate) fn displaced(&self, points: &VirtualPoints, ids: [&[[u32; 3]]; 2], offset_b: u32) -> PlaneRecords {
        let records = [0, 1].map(|operand| {
            let offset = if operand == 0 { 0 } else { offset_b };
            self.order[operand]
                .iter()
                .zip(&self.records[operand])
                .map(|(&triangle, g)| g.displaced(points, ids[operand][triangle as usize].map(|v| v + offset)))
                .collect::<Vec<_>>()
        });
        PlaneRecords { slot: self.slot.clone(), order: self.order.clone(), records }
    }

    /// The record of `triangle` of `operand`, which a pair names.
    pub(crate) fn get(&self, operand: usize, triangle: u32) -> &TrianglePlane {
        let slot = self.slot[operand][triangle as usize];
        debug_assert!(slot != u32::MAX, "a plane record is read only for a triangle a pair names");
        &self.records[operand][slot as usize]
    }
}

pub(crate) fn pair_sites(
    classes: &Classes,
    memo: &WindingMemo,
    planes: &PlaneRecords,
    a: PairTriangle,
    b: PairTriangle,
) -> PairSites {
    let points = &classes.points;
    // A class that holds all six ids makes every side 0 (orient3d answers so from the classes), so the pair is
    // coplanar without the six calls. The same test gives the coplanar path its shared axis.
    let shared_class = classes.triangle_class[0][a.index as usize].filter(|&class| {
        classes.triangle_class[1][b.index as usize] == Some(class)
            && a.ids.iter().chain(&b.ids).all(|&v| points.classes_of(v).contains(&class))
    });
    let (sa, sb) = if shared_class.is_some() {
        ([0; 3], [0; 3])
    } else {
        let (plane_a, plane_b) = (planes.get(0, a.index), planes.get(1, b.index));
        (
            a.ids.map(|v| {
                plane_b.side(points, v).unwrap_or_else(|| points.orient3d(b.ids[0], b.ids[1], b.ids[2], v))
            }),
            b.ids.map(|v| {
                plane_a.side(points, v).unwrap_or_else(|| points.orient3d(a.ids[0], a.ids[1], a.ids[2], v))
            }),
        )
    };
    let mut result = PairSites::default();
    result.coplanar = sa == [0; 3] && sb == [0; 3];
    if strict_sign(sa) || strict_sign(sb) {
        return result;
    }

    let triangles = [a, b];
    let (axes, windings) = if result.coplanar {
        let shared_axis = shared_class.map(|class| points.class(class).drop_axis());
        let axis = projection_axis(points, a, shared_axis);
        let windings = [0, 1].map(|i| {
            let [p, q, r] = triangles[i].ids;
            points.orient2d_along(axis, p, q, r)
        });
        ([axis, axis], windings)
    } else {
        // Not coplanar, so no class holds all six ids (every side would then be 0): `shared_axis` would be
        // None, and each triangle's axis and winding are its own.
        let (axis_a, winding_a) = memo.get(points, 0, a);
        let (axis_b, winding_b) = memo.get(points, 1, b);
        ([axis_a, axis_b], [winding_a, winding_b])
    };
    if windings.contains(&0) {
        result.broken += 1;
        return result;
    }

    if result.coplanar {
        let mut table = SignTable::new(points, axes[0], [a.ids, b.ids]);
        if table.apart(windings) {
            return result;
        }
        for operand in 0..2 {
            for e in 0..3 {
                clip_edge(&mut table, operand, e, windings[1 - operand], &mut result);
            }
        }
    } else {
        // Two triangles that do not meet give nothing: every classification below would return None.
        if let (Some(i), Some(j)) = (apex(sa), apex(sb)) {
            if apart_on_line(points, a.ids, i, sa[i], b.ids, j, sb[j]) {
                return result;
            }
        }
        let mut ends = Vec::new();
        for (operand, signs) in [(0, sa), (1, sb)] {
            let ids = triangles[operand].ids;
            let other = 1 - operand;
            for i in 0..3 {
                if signs[i] == 0 {
                    if let Some(key) = classify_vertex(
                        points, triangles[other], other as u8, axes[other],
                        windings[other], ids[i], &mut result,
                    ) {
                        ends.push(key);
                    }
                }
                let j = (i + 1) % 3;
                if signs[i] * signs[j] < 0 {
                    if let Some(key) = classify_edge(
                        points, triangles[other], operand as u8, [ids[i], ids[j]],
                        &mut result,
                    ) {
                        ends.push(key);
                    }
                }
            }
        }
        push_segment(&mut result, ends);
    }

    let mut i = 0;
    while i < result.on_edges.len() {
        if result.on_edges[..i].contains(&result.on_edges[i]) {
            result.on_edges.remove(i);
        } else {
            i += 1;
        }
    }
    result
}

fn projection_axis(points: &VirtualPoints, t: PairTriangle, shared: Option<usize>) -> usize {
    shared.unwrap_or_else(|| {
        let [p, q, r] = t.ids.map(|v| points.real(v));
        dominant_axis((q - p).cross(r - p))
    })
}

fn strict_sign(signs: [i32; 3]) -> bool {
    signs.iter().all(|&s| s > 0) || signs.iter().all(|&s| s < 0)
}

/// The first corner off the other's plane whose two neighbours lie on the closed opposite side: the sign
/// patterns (+,-,-), (+,0,-), (+,0,0), their rotations and their mirrors. None when one corner touches the
/// plane and the other two lie on one side, (0,+,+).
fn apex(signs: [i32; 3]) -> Option<usize> {
    (0..3).find(|&i| {
        signs[i] != 0 && signs[(i + 1) % 3] * signs[i] <= 0 && signs[(i + 2) % 3] * signs[i] <= 0
    })
}

/// Whether A (apex `i`, on side `sp` of B's plane) and B (apex `j`, side `su` of A's plane) are disjoint.
/// The planes cross in a line L. A meets B's plane in a segment of L whose ends lie on A's edges from the
/// apex P, to Q and to R; B likewise from U. Each test is one of `classify_edge`'s own signs, an edge of A
/// against an edge of B through their apexes, and reads "A's segment ends strictly before B's begins" or
/// the reverse along L. Either one separates the two segments, and A meet B lies in both. Signs are exact
/// and only strict ones reject, so triangles that touch go on.
fn apart_on_line(points: &VirtualPoints, a: [u32; 3], i: usize, sp: i32, b: [u32; 3], j: usize, su: i32) -> bool {
    let (p, q, r) = (a[i], a[(i + 1) % 3], a[(i + 2) % 3]);
    let (u, v, w) = (b[j], b[(j + 1) % 3], b[(j + 2) % 3]);
    if sp * su > 0 {
        sp * points.orient3d(p, q, u, v) > 0 || sp * points.orient3d(r, p, w, u) < 0
    } else {
        sp * points.orient3d(r, p, u, v) < 0 || sp * points.orient3d(p, q, w, u) > 0
    }
}

/// The signs of the coplanar path: `orient2d_along(axis, e0, e1, x)` for the cyclic edge e of triangle i,
/// (e0, e1) = (ids[i][e], ids[i][e + 1]), and corner x of the other triangle. Each is evaluated at most once,
/// and is 0 with no call when x is an end of the edge.
struct SignTable<'a> {
    points: &'a VirtualPoints,
    axis: usize,
    ids: [[u32; 3]; 2],
    /// 2 = not evaluated yet.
    signs: [[[i8; 3]; 3]; 2],
}

impl<'a> SignTable<'a> {
    fn new(points: &'a VirtualPoints, axis: usize, ids: [[u32; 3]; 2]) -> SignTable<'a> {
        SignTable { points, axis, ids, signs: [[[2; 3]; 3]; 2] }
    }

    fn get(&mut self, i: usize, e: usize, x: usize) -> i32 {
        if self.signs[i][e][x] == 2 {
            let (e0, e1, v) = (self.ids[i][e], self.ids[i][(e + 1) % 3], self.ids[1 - i][x]);
            self.signs[i][e][x] = if v == e0 || v == e1 {
                0
            } else {
                self.points.orient2d_along(self.axis, e0, e1, v) as i8
            };
        }
        i32::from(self.signs[i][e][x])
    }

    /// Whether the two triangles (windings `w`) meet in nothing, or only in their one shared id: some cyclic
    /// edge has every corner of the other that is not an end of it strictly outside. That edge's line keeps
    /// one triangle in its closed inner half-plane and the other, but for the shared id, in the open outer
    /// one. With one shared id s only the four edges through s are tried (B's two, then A's two) against the
    /// other's two corners besides s; with none, every edge against the other's three corners; with two or
    /// three shared ids nothing is tried.
    fn apart(&mut self, w: [i32; 2]) -> bool {
        let mut shared = 0;
        let mut s = 0;
        for &v in &self.ids[0] {
            if self.ids[1].contains(&v) {
                shared += 1;
                s = v;
            }
        }
        match shared {
            0 => {
                for i in 0..2 {
                    for e in 0..3 {
                        if (0..3).all(|x| w[i] * self.get(i, e, x) < 0) {
                            return true;
                        }
                    }
                }
                false
            }
            1 => {
                for i in [1, 0] {
                    let k = self.ids[i].iter().position(|&v| v == s).unwrap();
                    let other = self.ids[1 - i];
                    for e in [(k + 2) % 3, k] {
                        if (0..3).filter(|&x| other[x] != s).all(|x| w[i] * self.get(i, e, x) < 0) {
                            return true;
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }
}

fn ordered_edge([p, q]: [u32; 2]) -> [u32; 2] {
    [p.min(q), p.max(q)]
}

fn crossing_key(operand: u8, edge: [u32; 2], other: [u32; 2]) -> Key {
    if operand == 0 { Key::ee(edge, other) } else { Key::ee(other, edge) }
}

fn classify_vertex(
    points: &VirtualPoints,
    t: PairTriangle,
    operand: u8,
    axis: usize,
    winding: i32,
    v: u32,
    result: &mut PairSites,
) -> Option<Key> {
    if t.ids.contains(&v) {
        return Some(Key::V(v));
    }
    let signs = [0, 1, 2].map(|i| {
        winding * points.orient2d_along(axis, t.ids[i], t.ids[(i + 1) % 3], v)
    });
    vertex_in(t.ids, operand, v, signs, result)
}

/// Where v lies against triangle `ids` of `operand` from the three edge signs (winding applied): outside
/// (None), on a corner (counted broken), on edges (recorded) or inside.
fn vertex_in(ids: [u32; 3], operand: u8, v: u32, signs: [i32; 3], result: &mut PairSites) -> Option<Key> {
    let key = Key::V(v);
    if signs.iter().any(|&s| s < 0) {
        return None;
    }
    for i in 0..3 {
        if signs[i] == 0 && signs[(i + 2) % 3] == 0 {
            result.broken += 1;
            return Some(key);
        }
    }
    for i in 0..3 {
        if signs[i] == 0 {
            let edge = ordered_edge([ids[i], ids[(i + 1) % 3]]);
            result.on_edges.push((operand, edge, key));
        }
    }
    Some(key)
}

fn classify_edge(
    points: &VirtualPoints,
    t: PairTriangle,
    operand: u8,
    [p, q]: [u32; 2],
    result: &mut PairSites,
) -> Option<Key> {
    let signs = [0, 1, 2].map(|i| points.orient3d(p, q, t.ids[i], t.ids[(i + 1) % 3]));
    if strict_sign(signs) {
        return Some(Key::ef(operand, p, q, t.index));
    }
    for i in 0..3 {
        if signs[i] == 0 && signs[(i + 1) % 3] * signs[(i + 2) % 3] > 0 {
            return Some(crossing_key(operand, [p, q], [t.ids[i], t.ids[(i + 1) % 3]]));
        }
        if signs[i] == 0 && signs[(i + 2) % 3] == 0 && signs[(i + 1) % 3] != 0 {
            let key = Key::V(t.ids[i]);
            result.on_edges.push((operand, ordered_edge([p, q]), key));
            return Some(key);
        }
    }
    None
}

/// Clips edge e of triangle `operand` against the other triangle (winding `winding`), on the coplanar path.
fn clip_edge(table: &mut SignTable, operand: usize, e: usize, winding: i32, result: &mut PairSites) {
    let other = 1 - operand;
    let (ids, t) = (table.ids[operand], table.ids[other]);
    let (xp, xq) = (e, (e + 1) % 3);
    let (p, q) = (ids[xp], ids[xq]);
    let mut ends = Vec::new();
    for x in [xp, xq] {
        let v = ids[x];
        let key = if t.contains(&v) {
            Some(Key::V(v))
        } else {
            let signs = [0, 1, 2].map(|i| winding * table.get(other, i, x));
            vertex_in(t, other as u8, v, signs, result)
        };
        if let Some(key) = key {
            ends.push(key);
        }
    }
    let edge = ordered_edge([p, q]);
    for i in 0..3 {
        let u = t[i];
        let v = t[(i + 1) % 3];
        let sp = winding * table.get(other, i, xp);
        let sq = winding * table.get(other, i, xq);
        if sp * sq < 0 {
            let ou = table.get(operand, e, i);
            let ov = table.get(operand, e, (i + 1) % 3);
            if ou * ov < 0 {
                ends.push(crossing_key(operand as u8, edge, [u, v]));
            }
            for (id, sign) in [(u, ou), (v, ov)] {
                if sign == 0 {
                    let key = Key::V(id);
                    ends.push(key);
                    result.on_edges.push((operand as u8, edge, key));
                }
            }
        } else if sp == 0 && sq == 0 {
            let points = table.points;
            let along = dominant_axis(points.real(q) - points.real(p));
            for id in [u, v] {
                let order = points.compare(along, p, id);
                if order != Ordering::Equal && order == points.compare(along, id, q) {
                    let key = Key::V(id);
                    ends.push(key);
                    result.on_edges.push((operand as u8, edge, key));
                }
            }
        }
    }
    push_segment(result, ends);
}

fn push_segment(result: &mut PairSites, mut ends: Vec<Key>) {
    ends.sort_unstable();
    ends.dedup();
    match ends.as_slice() {
        [] | [_] => {}
        &[a, b] => {
            let segment = [a, b];
            if !result.segments.contains(&segment) {
                result.segments.push(segment);
            }
        }
        _ => result.broken += 1,
    }
}
