use crate::cdt::{Point2, Predicates, CDT};
use crate::keys::{compare_along, dominant_axis, edge_edge, edge_plane, Constructed, Key};
use crate::virtual_points::{determinant3, determinant4, VirtualPoints};
use makepad_csg_exact::{two_diff, two_product, two_sum, Dyadic};
use makepad_csg_math::fast_hash::{FastMap, FastSet};
use makepad_csg_math::Vec3d;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;

/// Every point the cut uses, by id.
pub(crate) struct KeyPool {
    vertex_count: u32,
    keys: Vec<Key>,
    /// The constructed points of `keys`, once [`KeyPool::construct`] has made them.
    points: Vec<Constructed>,
    ids: FastMap<Key, u32>,
}

impl KeyPool {
    pub(crate) fn new(vertex_count: u32) -> KeyPool {
        KeyPool {
            vertex_count,
            keys: Vec::new(),
            points: Vec::new(),
            ids: FastMap::default(),
        }
    }

    /// The id of a key, new keys numbered in first-use order; their points are made later, together.
    pub(crate) fn id(&mut self, key: Key) -> u32 {
        if let Key::V(v) = key {
            debug_assert!(v < self.vertex_count);
            return v;
        }
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = self.len();
        self.keys.push(key);
        self.ids.insert(key, id);
        id
    }

    /// The keys that need a constructed point, in id order.
    pub(crate) fn constructed_keys(&self) -> &[Key] {
        &self.keys
    }

    /// Take the constructed points of [`KeyPool::constructed_keys`], in the same order.
    pub(crate) fn set_points(&mut self, points: Vec<Constructed>) {
        assert_eq!(points.len(), self.keys.len(), "one constructed point per key");
        self.points = points;
    }

    /// The number of ids, including input vertices.
    pub(crate) fn len(&self) -> u32 {
        self.vertex_count + self.keys.len() as u32
    }

    pub(crate) fn key(&self, id: u32) -> Key {
        if id < self.vertex_count {
            Key::V(id)
        } else {
            self.keys[(id - self.vertex_count) as usize]
        }
    }

    /// The exact homogeneous coordinates of an id.
    pub(crate) fn coordinates(&self, points: &VirtualPoints, id: u32) -> [Dyadic; 4] {
        if id < self.vertex_count {
            points.homogeneous(id)
        } else {
            self.points[(id - self.vertex_count) as usize].coordinates.clone()
        }
    }

    /// The constructed point of an id, or None for an input vertex.
    pub(crate) fn constructed(&self, id: u32) -> Option<&Constructed> {
        id.checked_sub(self.vertex_count).map(|index| &self.points[index as usize])
    }

    /// Every id's output position, in id order.
    pub(crate) fn positions(&self, points: &VirtualPoints) -> Vec<Vec3d> {
        (0..self.vertex_count)
            .map(|id| points.approx(id))
            .chain(self.points.iter().map(|point| point.position))
            .collect()
    }
}

/// The exact point of a constructed key; triangles are A's then B's, with aliases applied.
pub(crate) fn construct(points: &VirtualPoints, triangles: [&[[u32; 3]]; 2], key: Key) -> Constructed {
    match key {
        Key::EF(operand, [p, q], triangle) => {
            edge_plane(points, p, q, triangles[1 - operand as usize][triangle as usize])
        }
        Key::EE([a0, a1], [b0, b1]) => {
            let axis = dominant_axis(
                (points.approx(a1) - points.approx(a0))
                    .cross(points.approx(b1) - points.approx(b0)),
            );
            edge_edge(points, a0, a1, b0, b1, axis)
        }
        Key::V(_) => unreachable!("input vertices are not constructed"),
    }
}

/// The ids strictly inside each edge, ordered from its lower id's end.
pub(crate) struct EdgeTable {
    edges: FastMap<[u32; 2], Vec<u32>>,
    /// Per input vertex, whether an edge of the table ends there.
    ends: Vec<bool>,
    pub(crate) ties: usize,
}

impl EdgeTable {
    /// Combine input vertices on edges with every constructed key's own edges.
    pub(crate) fn build(
        pool: &KeyPool,
        points: &VirtualPoints,
        on_edges: &[([u32; 2], u32)],
    ) -> EdgeTable {
        let mut edges: FastMap<[u32; 2], Vec<u32>> = FastMap::default();
        let mut add = |edge: [u32; 2], id: u32| {
            let edge = ordered_edge(edge[0], edge[1]);
            if !edge.contains(&id) {
                edges.entry(edge).or_default().push(id);
            }
        };
        for &(edge, id) in on_edges {
            add(edge, id);
        }
        for id in pool.vertex_count..pool.len() {
            match pool.key(id) {
                Key::EF(_, edge, _) => add(edge, id),
                Key::EE(a, b) => {
                    add(a, id);
                    add(b, id);
                }
                Key::V(_) => unreachable!(),
            }
        }

        let mut ties = 0;
        // Positions round within 2^-50 of each coordinate, so a float gap past 2^-48 of the two
        // magnitudes is the exact order; closer points compare exactly.
        let position = |id: u32| if id < pool.vertex_count {
            points.approx(id)
        } else {
            pool.points[(id - pool.vertex_count) as usize].position
        };
        let along = |axis: usize, a: u32, b: u32| {
            let (x, y) = (position(a).component(axis), position(b).component(axis));
            if (x - y).abs() > 2.0_f64.powi(-48) * (x.abs() + y.abs()) {
                return x.total_cmp(&y);
            }
            compare_along(axis, &pool.coordinates(points, a), &pool.coordinates(points, b))
        };
        for (&[a, b], ids) in &mut edges {
            ids.sort_unstable();
            ids.dedup();
            let axis = dominant_axis(points.approx(b) - points.approx(a));
            let direction = along(axis, a, b);
            ids.sort_unstable_by(|&p, &q| {
                let order = along(axis, p, q);
                let order = if direction == Ordering::Greater { order.reverse() } else { order };
                order.then_with(|| pool.key(p).cmp(&pool.key(q)))
            });
            // Count adjacent equal points once, independently of the sort's comparisons.
            ties += ids.windows(2)
                .filter(|pair| along(axis, pair[0], pair[1]) == Ordering::Equal)
                .count();
        }
        let mut ends = vec![false; pool.vertex_count as usize];
        for &[a, b] in edges.keys() {
            ends[a as usize] = true;
            ends[b as usize] = true;
        }
        EdgeTable { edges, ends, ties }
    }

    /// At least two of the corners end an edge of the table: only then can the triangle's edges carry points.
    pub(crate) fn touches(&self, corners: [u32; 3]) -> bool {
        corners.iter().filter(|&&v| self.ends[v as usize]).count() >= 2
    }

    /// The runs of a triangle's three edges, in its corner order; most triangles have none, which
    /// two corners without a table edge show before any lookup.
    pub(crate) fn runs(&self, corners: [u32; 3]) -> [Vec<u32>; 3] {
        if !self.touches(corners) {
            return Default::default();
        }
        std::array::from_fn(|i| self.run(corners[i], corners[(i + 1) % 3]))
    }

    /// The ids strictly inside an edge, in order from `from`.
    pub(crate) fn run(&self, from: u32, to: u32) -> Vec<u32> {
        let mut run = self.edges.get(&ordered_edge(from, to)).cloned().unwrap_or_default();
        if from > to {
            run.reverse();
        }
        run
    }
}

fn ordered_edge(a: u32, b: u32) -> [u32; 2] {
    [a.min(b), a.max(b)]
}

struct ExactPredicates<'a> {
    ids: &'a [u32],
    /// Per CDT point, the triangle edges it lies on (bit i: corner i to corner i + 1): three points
    /// sharing one lie on one 3D line, so they are exactly collinear in any projection.
    edges: Vec<u8>,
    axes: [usize; 2],
    coordinates: &'a (dyn Fn(u32) -> [Dyadic; 4] + Sync),
    /// Per CDT point, filled on first use: its exact homogeneous (x, y, w), and the tails of x / w and
    /// y / w after the rounded position the CDT holds.
    cache: RefCell<Vec<Option<([Dyadic; 3], [f64; 2])>>>,
    exact: &'a Cell<usize>,
}

impl ExactPredicates<'_> {
    fn scale(points: &[Point2]) -> (f64, f64) {
        let mut min = points[0];
        let mut max = min;
        let mut m = 0.0_f64;
        for p in points {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            m = m.max(p.x.abs()).max(p.y.abs());
        }
        ((max.x - min.x).max(max.y - min.y), m)
    }

    /// Fills point i's cache row. Its tails are the remainders x / w - hx and y / w - hy after its
    /// rounded position (hx, hy), as doubles; a tail over 2^-49 of its coordinate (the position is not
    /// the rounded quotient) is NaN, which fails every double-double comparison.
    fn fill(&self, cache: &mut [Option<([Dyadic; 3], [f64; 2])>], points: &[Point2], i: u32) {
        cache[i as usize].get_or_insert_with(|| {
            let p = points[i as usize];
            if i < 3 {
                return ([Dyadic::from_f64(p.x), Dyadic::from_f64(p.y), Dyadic::from_f64(1.0)], [0.0; 2]);
            }
            let c = (self.coordinates)(self.ids[(i - 3) as usize]);
            let row = [c[self.axes[0]].clone(), c[self.axes[1]].clone(), c[3].clone()];
            let tail = |x: &Dyadic, h: f64| {
                let t = x.sub(&Dyadic::from_f64(h).mul(&row[2])).ratio(&row[2]);
                if t.abs() <= 2.0_f64.powi(-49) * h.abs() { t } else { f64::NAN }
            };
            let tails = [tail(&row[0], p.x), tail(&row[1], p.y)];
            (row, tails)
        });
    }

    fn evaluate<const N: usize, R>(
        &self,
        points: &[Point2],
        indices: [u32; N],
        f: impl FnOnce([&[Dyadic; 3]; N]) -> R,
    ) -> R {
        let mut cache = self.cache.borrow_mut();
        for &i in &indices {
            self.fill(&mut cache, points, i);
        }
        self.exact.set(self.exact.get() + 1);
        f(indices.map(|i| &cache[i as usize].as_ref().unwrap().0))
    }

    /// The orientation where it is certain without an exact evaluation: 0 for three points on one
    /// triangle edge, the plain determinant past its bound; None otherwise.
    fn orient_filtered(&self, points: &[Point2], a: u32, b: u32, c: u32) -> Option<f64> {
        if self.edges[a as usize] & self.edges[b as usize] & self.edges[c as usize] != 0 {
            return Some(0.0);
        }
        let [pa, pb, pc] = [a, b, c].map(|i| points[i as usize]);
        // The plain determinant: below the threshold the exact answer is asked anyway, so a robust
        // float fallback (an exact answer about the rounded positions) would only be wasted work.
        let d = (pa.x - pc.x) * (pb.y - pc.y) - (pa.y - pc.y) * (pb.x - pc.x);
        let (s, m) = Self::scale(&[pa, pb, pc]);
        // The filter includes the error from rounding exact projected coordinates.
        if d.abs() > 2.0_f64.powi(-45) * s * m {
            return Some(d);
        }
        None
    }

    /// The in-circle determinant where its sign is certain without an exact evaluation, else None.
    fn in_circle_filtered(&self, points: &[Point2], a: u32, b: u32, c: u32, d: u32) -> Option<f64> {
        let [pa, pb, pc, pd] = [a, b, c, d].map(|i| points[i as usize]);
        let det = in_circle_float(pa, pb, pc, pd);
        let (s, m) = Self::scale(&[pa, pb, pc, pd]);
        if det.abs() > 2.0_f64.powi(-42) * s * s * s * m {
            return Some(det);
        }
        None
    }

    /// The in-circle sign past the float filter: the double-double determinant where its bound makes
    /// the sign certain, else the exact sign (-1, 0 or 1).
    fn in_circle_decided(&self, points: &[Point2], a: u32, b: u32, c: u32, d: u32) -> f64 {
        let tails = {
            let mut cache = self.cache.borrow_mut();
            [a, b, c, d].map(|i| {
                self.fill(&mut cache, points, i);
                cache[i as usize].as_ref().unwrap().1
            })
        };
        if let Some(det) = in_circle_double_double([a, b, c, d].map(|i| points[i as usize]), tails) {
            debug_assert_eq!(det.signum(), {
                let cache = self.cache.borrow();
                in_circle_sign([a, b, c, d].map(|i| &cache[i as usize].as_ref().unwrap().0))
            }, "the double-double in-circle sign is the exact one");
            return det;
        }
        self.evaluate(points, [a, b, c, d], in_circle_sign)
    }
}

/// The exact in-circle sign (-1, 0 or 1) of four homogeneous rows.
fn in_circle_sign(rows: [&[Dyadic; 3]; 4]) -> f64 {
    // Positive squared weights clear the denominators without changing the sign.
    let rows = rows.map(|[x, y, w]| [
        x.mul(w), y.mul(w), x.mul(x).add(&y.mul(y)), w.mul(w),
    ]);
    determinant4(rows.each_ref()).sign() as f64
}

/// The in-circle determinant of `d` against `a`, `b`, `c` in plain doubles (Shewchuk's first stage
/// without its error bound).
fn in_circle_float(a: Point2, b: Point2, c: Point2, d: Point2) -> f64 {
    let (adx, ady) = (a.x - d.x, a.y - d.y);
    let (bdx, bdy) = (b.x - d.x, b.y - d.y);
    let (cdx, cdy) = (c.x - d.x, c.y - d.y);
    let abdet = adx * bdy - bdx * ady;
    let bcdet = bdx * cdy - cdx * bdy;
    let cadet = cdx * ady - adx * cdy;
    let alift = adx * adx + ady * ady;
    let blift = bdx * bdx + bdy * bdy;
    let clift = cdx * cdx + cdy * cdy;
    alift * bcdet + blift * cadet + clift * abdet
}

// Double-double values (hi, lo): exact two_sum, two_diff and two_product (Dekker's split, no fused
// multiply-add: the same bits on every target), each operation closed by a two_sum, which keeps its
// bound under cancellation.
fn dd_mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (p, e) = two_product(a.0, b.0);
    two_sum(p, e + (a.0 * b.1 + a.1 * b.0))
}

fn dd_add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (s, e) = two_sum(a.0, b.0);
    two_sum(s, e + (a.1 + b.1))
}

fn dd_sub(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    dd_add(a, (-b.0, -b.1))
}

/// `in_circle_float`'s determinant in double-double on each point's rounded position plus its tail,
/// where its sign is certain; else None.
///
/// A tail is within 2^-50 of itself of the exact remainder, so each coordinate is within 2^-99 m of
/// exact, m the largest coordinate. With S = s + 2^-47 m, which bounds every difference, exact or
/// computed, the data error is under 2^-92.1 S^3 m and the evaluation error under 2^-95 S^3 m: the
/// bound 2^-90 S^3 m keeps 3.8x of slack. A NaN tail fails the comparison and falls through.
fn in_circle_double_double(p: [Point2; 4], tails: [[f64; 2]; 4]) -> Option<f64> {
    let (s, m) = ExactPredicates::scale(&p);
    let size = s + 2.0_f64.powi(-47) * m;
    let bound = 2.0_f64.powi(-90) * size * size * size * m;
    // Below 2^100 no product overflows; above 2^-700 underflows stay far inside the slack.
    if !(m < 2.0_f64.powi(100) && bound > 2.0_f64.powi(-700)) {
        return None;
    }
    let diff = |i: usize, axis: usize| {
        let [h, k] = [p[i], p[3]].map(|q| if axis == 0 { q.x } else { q.y });
        let (h, e) = two_diff(h, k);
        two_sum(h, e + (tails[i][axis] - tails[3][axis]))
    };
    let (adx, ady) = (diff(0, 0), diff(0, 1));
    let (bdx, bdy) = (diff(1, 0), diff(1, 1));
    let (cdx, cdy) = (diff(2, 0), diff(2, 1));
    let abdet = dd_sub(dd_mul(adx, bdy), dd_mul(bdx, ady));
    let bcdet = dd_sub(dd_mul(bdx, cdy), dd_mul(cdx, bdy));
    let cadet = dd_sub(dd_mul(cdx, ady), dd_mul(adx, cdy));
    let alift = dd_add(dd_mul(adx, adx), dd_mul(ady, ady));
    let blift = dd_add(dd_mul(bdx, bdx), dd_mul(bdy, bdy));
    let clift = dd_add(dd_mul(cdx, cdx), dd_mul(cdy, cdy));
    let det = dd_add(dd_add(dd_mul(alift, bcdet), dd_mul(blift, cadet)), dd_mul(clift, abdet));
    if !(det.0.abs() > bound) {
        return None;
    }
    Some(det.0)
}

impl Predicates for ExactPredicates<'_> {
    fn orient(&self, points: &[Point2], a: u32, b: u32, c: u32) -> f64 {
        if let Some(d) = self.orient_filtered(points, a, b, c) {
            return d;
        }
        self.evaluate(points, [a, b, c], |[a, b, c]| {
            determinant3(a.each_ref(), b.each_ref(), c.each_ref()).sign() as f64
        })
    }

    fn in_circle(&self, points: &[Point2], a: u32, b: u32, c: u32, d: u32) -> f64 {
        self.in_circle_filtered(points, a, b, c, d)
            .unwrap_or_else(|| self.in_circle_decided(points, a, b, c, d))
    }

    fn flips(&self, points: &[Point2], a: u32, b: u32, p: u32, d: u32) -> bool {
        if let Some(det) = self.in_circle_filtered(points, a, b, p, d) {
            return det > 0.0;
        }
        // d strictly inside beyond the chord makes the quad a, d, b, p strictly convex: a corner
        // where it certainly does not turn left means no flip, without the exact sign. An uncertain
        // corner asks the exact in-circle sign, not the exact orientation.
        let left =
            |x: u32, y: u32, z: u32| self.orient_filtered(points, x, y, z).map_or(true, |o| o > 0.0);
        if !left(p, a, d) || !left(p, d, b) {
            return false;
        }
        self.in_circle_decided(points, a, b, p, d) > 0.0
    }

    fn same(&self, points: &[Point2], a: u32, b: u32) -> bool {
        let [pa, pb] = [a, b].map(|i| points[i as usize]);
        let (_, m) = Self::scale(&[pa, pb]);
        // Positions round within 2^-50 relative: farther apart, they are two points.
        if (pa.x - pb.x).abs() > 2.0_f64.powi(-48) * m || (pa.y - pb.y).abs() > 2.0_f64.powi(-48) * m {
            return false;
        }
        self.evaluate(points, [a, b], |[a, b]| {
            a[0].mul(&b[2]) == b[0].mul(&a[2])
                && a[1].mul(&b[2]) == b[1].mul(&a[2])
        })
    }
}

/// One triangle to cut.
pub(crate) struct CutTriangle {
    /// Its corners' ids, in its winding order.
    pub(crate) corners: [u32; 3],
    /// The ids strictly inside each corner-to-next-corner edge, in that order.
    pub(crate) runs: [Vec<u32>; 3],
    /// Id pairs that must become edges of its pieces, deduplicated by the caller.
    pub(crate) segments: Vec<[u32; 2]>,
    /// Its real input corners, for the projection.
    pub(crate) real: [Vec3d; 3],
}

#[derive(Default)]
pub(crate) struct Cut {
    /// The pieces, as ids, in the triangle's winding.
    pub(crate) pieces: Vec<[u32; 3]>,
    /// Exact predicate evaluations in the cut's triangulation.
    pub(crate) exact: usize,
    /// The pieces' directed boundary differs from the chain cycle, or an edge repeats.
    pub(crate) boundary: bool,
    /// Added segment constraints absent from the pieces' edges.
    pub(crate) missing: usize,
    /// The real normal is zero, so the triangle is returned uncut.
    pub(crate) degenerate: bool,
    /// The pieces' edges along the segments, as ascending id pairs: where the triangle meets the
    /// other operand, which classification does not flood across.
    pub(crate) walls: Vec<[u32; 2]>,
}

/// Cut one triangle along its chain and segments; positions are indexed by point id.
pub(crate) fn cut_triangle(
    triangle: &CutTriangle,
    positions: &[Vec3d],
    coordinates: &(dyn Fn(u32) -> [Dyadic; 4] + Sync),
) -> Cut {
    let normal = (triangle.real[1] - triangle.real[0])
        .cross(triangle.real[2] - triangle.real[0]);
    if normal.x == 0.0 && normal.y == 0.0 && normal.z == 0.0 {
        return Cut {
            pieces: vec![triangle.corners],
            degenerate: true,
            ..Cut::default()
        };
    }
    let axis = dominant_axis(normal);
    let project = |id: u32| {
        let p = positions[id as usize];
        [p.component((axis + 1) % 3), p.component((axis + 2) % 3)]
    };
    let mut result = Cut::default();
    let mut ids = Vec::new();
    let mut projected = Vec::new();
    let mut membership: FastMap<u32, u8> = FastMap::default();
    for i in 0..3 {
        let corner = triangle.corners[i];
        ids.push(corner);
        projected.push(project(corner));
        *membership.entry(corner).or_default() |= (1 << i) | (1 << ((i + 2) % 3));
        for &id in &triangle.runs[i] {
            ids.push(id);
            projected.push(project(id));
            *membership.entry(id).or_default() |= 1 << i;
        }
    }
    let chain_len = ids.len();
    let mut local: FastMap<u32, usize> = ids.iter().enumerate()
        .map(|(i, &id)| (id, i))
        .collect();
    for segment in &triangle.segments {
        for &id in segment {
            if !local.contains_key(&id) {
                local.insert(id, ids.len());
                ids.push(id);
                projected.push(project(id));
            }
        }
    }

    let mut min = Point2 { x: f64::INFINITY, y: f64::INFINITY };
    let mut max = Point2 { x: f64::NEG_INFINITY, y: f64::NEG_INFINITY };
    for &[x, y] in &projected {
        min.x = min.x.min(x);
        min.y = min.y.min(y);
        max.x = max.x.max(x);
        max.y = max.y.max(y);
    }
    let exact = Cell::new(0);
    let edges = [0; 3].into_iter()
        .chain(ids.iter().map(|id| membership.get(id).copied().unwrap_or(0)))
        .collect();
    let predicates = ExactPredicates {
        ids: &ids,
        edges,
        axes: [(axis + 1) % 3, (axis + 2) % 3],
        coordinates,
        cache: RefCell::new(vec![None; ids.len() + 3]),
        exact: &exact,
    };
    let mut cdt = CDT::with_predicates(min, max, predicates);
    let indices: Vec<_> = projected.iter()
        .map(|&[x, y]| cdt.insert_point(x, y))
        .collect();
    let mut walls = Vec::with_capacity(chain_len);
    for i in 0..chain_len {
        let wall = (indices[i], indices[(i + 1) % chain_len]);
        cdt.add_constraint(wall.0, wall.1);
        walls.push(wall);
    }
    let mut constraints = Vec::new();
    let ascending = |a: u32, b: u32| [a.min(b), a.max(b)];
    for &[a, b] in &triangle.segments {
        let common = membership.get(&a).copied().unwrap_or(0)
            & membership.get(&b).copied().unwrap_or(0);
        if common == 0 {
            cdt.add_constraint(indices[local[&a]], indices[local[&b]]);
            constraints.push([a, b]);
        } else {
            // A segment along an edge covers the chain between its ends, one way round or the other.
            let (from, to) = (local[&a], local[&b]);
            let on_edge = |start: usize, end: usize| {
                let steps = (end + chain_len - start) % chain_len;
                (1..steps).all(|step| membership[&ids[(start + step) % chain_len]] & common != 0)
                    .then_some(steps)
            };
            let (start, steps) = match on_edge(from, to) {
                Some(steps) => (from, steps),
                None => (to, on_edge(to, from).unwrap_or(0)),
            };
            result.walls.extend((0..steps).map(|step| {
                ascending(ids[(start + step) % chain_len], ids[(start + step + 1) % chain_len])
            }));
        }
    }
    cdt.finalize();
    let cycle: FastSet<_> = (0..chain_len)
        .map(|i| ascending(ids[i], ids[(i + 1) % chain_len]))
        .collect();
    result.walls.extend(cdt.constrained_edges().into_iter()
        .map(|(a, b)| ascending(ids[a as usize], ids[b as usize]))
        .filter(|edge| !cycle.contains(edge)));
    for piece in cdt.inside(&walls) {
        let mut piece = piece.map(|i| ids[i as usize]);
        // CDT triangles are counterclockwise in the cyclic projection frame.
        if normal.component(axis) < 0.0 {
            piece.swap(1, 2);
        }
        result.pieces.push(piece);
    }
    // Each piece from its least id, the pieces sorted: what follows depends on the triangulation only,
    // not on how the CDT stores it.
    for piece in &mut result.pieces {
        let first = (0..3).min_by_key(|&i| piece[i]).unwrap_or(0);
        piece.rotate_left(first);
    }
    result.pieces.sort_unstable();
    result.exact = exact.get();

    let mut directed = FastSet::default();
    for piece in &result.pieces {
        for i in 0..3 {
            if !directed.insert((piece[i], piece[(i + 1) % 3])) {
                result.boundary = true;
            }
        }
    }
    let boundary: FastSet<_> = directed.iter().copied()
        .filter(|&(a, b)| !directed.contains(&(b, a)))
        .collect();
    let cycle: FastSet<_> = (0..chain_len)
        .map(|i| (ids[i], ids[(i + 1) % chain_len]))
        .collect();
    result.boundary |= boundary != cycle || cycle.len() != chain_len;
    result.missing = constraints.iter()
        .filter(|&&[a, b]| !directed.contains(&(a, b)) && !directed.contains(&(b, a)))
        .count();
    result
}
