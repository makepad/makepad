//! Half-edge LOD simplification with certified two-sided Hausdorff bounds to
//! the input surface per group, and texture deviation bounds for textured groups.
//! Every level's positions and wedges always refer to the immutable input.
//! The subdivision floor refuses a collapse it cannot certify, never errors.

use crate::spatial::{BinGrid, BIN_DILATE, BIN_SIZE, GRID_RES};
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap};

pub struct SimplifyInput<'a> {
    /// One entry per welded source vertex; one call simplifies one part.
    pub positions: &'a [[f64; 3]],
    pub triangles: &'a [[u32; 3]],
    /// Caller-defined continuity ids, one per triangle corner.
    pub wedges: &'a [[u32; 3]],
    pub wedge_uv: &'a [[f64; 2]],
    pub groups: &'a [u32],
    /// Whether each group samples a texture.
    pub uv_bound: &'a [bool],
}

pub struct SimplifyOutput {
    /// (Position index, wedge id), both referring to the input.
    pub triangles: Vec<[(u32, u32); 3]>,
    pub groups: Vec<u32>,
    pub report: SimplifyReport,
}

pub struct SimplifyLevel {
    pub max_error: f64,
    pub target_faces: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct SimplifyReport {
    pub faces: usize,
    /// Certified forward upper bound, a running maximum over all collapses
    /// through this level, in input units.
    pub max_forward: f64,
    /// Certified backward upper bound, a running maximum including texture
    /// deviation and source-curve distance where applicable, in input units.
    pub max_backward: f64,
    pub stop: SimplifyStop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimplifyStop {
    Target,
    NoCandidate,
}

type Point = [f64; 3];
type Corners = [(u32, u32); 3];
type Edge = (u32, u32);

fn edge(a: u32, b: u32) -> Edge {
    (a.min(b), a.max(b))
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn mix(t: [Point; 3], w: [f64; 3]) -> Point {
    std::array::from_fn(|a| t[0][a] * w[0] + t[1][a] * w[1] + t[2][a] * w[2])
}

fn normal(t: [Point; 3]) -> Point {
    cross(sub(t[1], t[0]), sub(t[2], t[0]))
}

/// A face a collapse creates must stand clear of its longest edge by this
/// fraction of that edge. Mesh validation refuses a face whose area is within
/// 1e-12 of its extent squared, and a cut line leaves rows of collinear source
/// vertices that a collapse can join into such a face; the margin covers the
/// caller's round trip back to object space.
const MIN_HEIGHT_RATIO: f64 = 1e-9;

/// `n` is the triangle's unnormalized normal: its length is the longest edge
/// times the height over it.
fn sliver(t: [Point; 3], n: Point) -> bool {
    let longest = [sub(t[1], t[0]), sub(t[2], t[1]), sub(t[0], t[2])]
        .into_iter()
        .map(|e| dot(e, e))
        .fold(0.0, f64::max);
    dot(n, n) <= (MIN_HEIGHT_RATIO * longest).powi(2)
}

fn segment_distance_sq(p: Point, a: Point, b: Point) -> f64 {
    let ab = sub(b, a);
    let ap = sub(p, a);
    let length_sq = dot(ab, ab);
    let t = if length_sq > 0.0 {
        (dot(ap, ab) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let d = std::array::from_fn(|i| ap[i] - t * ab[i]);
    dot(d, d)
}

fn distance_sq(p: Point, t: [Point; 3]) -> f64 {
    let n = normal(t);
    let nn = dot(n, n);
    if nn > 0.0 && (0..3).all(|i| dot(cross(sub(t[(i + 1) % 3], t[i]), sub(p, t[i])), n) >= 0.0) {
        let d = dot(sub(p, t[0]), n);
        d * d / nn
    } else {
        (0..3)
            .map(|i| segment_distance_sq(p, t[i], t[(i + 1) % 3]))
            .fold(f64::INFINITY, f64::min)
    }
}

/// The distance from a point to a fixed set of triangles.
trait Field {
    /// The nearest distance and triangle id, or None when none is within reach.
    /// Err(()) cancels.
    fn nearest(&mut self, p: Point) -> Result<Option<(f64, usize)>, ()>;
    /// The distance from `p` to triangle `id` of the set.
    fn distance(&mut self, p: Point, id: usize) -> f64;
}

/// Sound: every point of `t` is within the returned `u <= bound` of the set.
/// Complete to (15/16) bound: it accepts every triangle whose true distance is
/// at most (15/16) bound, since a floor cell has U_lip <= max f + bound/16.
/// The floor refuses one collapse, never errors; `ctl` runs through `nearest`
/// so cancellation works throughout subdivision.
fn certify(
    t: [Point; 3],
    bound: f64,
    field: &mut dyn Field,
    witnesses: &mut Vec<usize>,
) -> Result<Option<f64>, ()> {
    #[derive(Clone, Copy)]
    struct Corner {
        p: Point,
        f: f64,
        id: usize,
    }

    fn query(p: Point, bound: f64, field: &mut dyn Field) -> Result<Option<Corner>, ()> {
        Ok(match field.nearest(p)? {
            Some((f, id)) if f <= bound => Some(Corner { p, f, id }),
            _ => None,
        })
    }

    witnesses.clear();
    let mut root = [Corner { p: [0.0; 3], f: 0.0, id: 0 }; 3];
    for (corner, p) in root.iter_mut().zip(t) {
        let Some(value) = query(p, bound, field)? else {
            return Ok(None);
        };
        *corner = value;
    }
    let mut stack = vec![root];
    let mut upper: f64 = 0.0;
    while let Some(cell) = stack.pop() {
        let longest = (0..3)
            .map(|i| {
                let e = sub(cell[i].p, cell[(i + 1) % 3].p);
                dot(e, e).sqrt()
            })
            .fold(0.0, f64::max);
        let radius = longest / 3f64.sqrt();
        let lip = cell.iter().map(|c| c.f).fold(0.0, f64::max) + radius;
        let mut ids = cell.map(|c| c.id);
        ids.sort_unstable();
        let mut convex = f64::INFINITY;
        for i in 0..3 {
            if i > 0 && ids[i] == ids[i - 1] {
                continue;
            }
            let id = ids[i];
            let farthest = cell.iter().map(|c| {
                if c.id == id { c.f } else { field.distance(c.p, id) }
            }).fold(0.0, f64::max);
            convex = convex.min(farthest);
        }
        let u = lip.min(convex);
        if u <= bound {
            upper = upper.max(u);
            witnesses.extend_from_slice(&ids);
        } else if bound == 0.0 || radius <= bound / 16.0 {
            return Ok(None);
        } else {
            let mut mid = cell;
            for i in 0..3 {
                let p = std::array::from_fn(|a| (cell[i].p[a] + cell[(i + 1) % 3].p[a]) * 0.5);
                let Some(value) = query(p, bound, field)? else {
                    return Ok(None);
                };
                mid[i] = value;
            }
            // Each midpoint is queried once and shared by all four children.
            stack.extend([
                [cell[0], mid[0], mid[2]],
                [mid[0], cell[1], mid[1]],
                [mid[2], mid[1], cell[2]],
                [mid[0], mid[1], mid[2]],
            ]);
        }
    }
    witnesses.sort_unstable();
    witnesses.dedup();
    Ok(Some(upper))
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Affine UV weights, without clamping to the triangle.
fn uv_weights(p: [f64; 2], t: [[f64; 2]; 3]) -> Option<[f64; 3]> {
    let det = orient(t[0], t[1], t[2]);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let w = [
        orient(t[1], t[2], p) / det,
        orient(t[2], t[0], p) / det,
        orient(t[0], t[1], p) / det,
    ];
    w.iter().all(|v| v.is_finite()).then_some(w)
}

fn uv_overlap(mut subject: [[f64; 2]; 3], mut clip: [[f64; 2]; 3]) -> Vec<[f64; 2]> {
    if orient(subject[0], subject[1], subject[2]) < 0.0 {
        subject.swap(1, 2);
    }
    if orient(clip[0], clip[1], clip[2]) < 0.0 {
        clip.swap(1, 2);
    }
    let mut polygon = subject.to_vec();
    let mut output = Vec::new();
    for i in 0..3 {
        let Some(&last) = polygon.last() else {
            break;
        };
        let (a, b) = (clip[i], clip[(i + 1) % 3]);
        let mut previous = last;
        let mut previous_side = orient(a, b, previous);
        for &p in &polygon {
            let side = orient(a, b, p);
            if (side >= 0.0) != (previous_side >= 0.0) {
                let t = previous_side / (previous_side - side);
                output.push(std::array::from_fn(|a| previous[a] + t * (p[a] - previous[a])));
            }
            if side >= 0.0 {
                output.push(p);
            }
            previous = p;
            previous_side = side;
        }
        std::mem::swap(&mut polygon, &mut output);
        output.clear();
    }
    polygon
}

fn uv_area(polygon: &[[f64; 2]]) -> f64 {
    (1..polygon.len().saturating_sub(1))
        .map(|i| orient(polygon[0], polygon[i], polygon[i + 1]))
        .sum::<f64>()
        .abs() * 0.5
}

#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: Point, p: Point, weight: f64) -> Self {
        let v = [n[0], n[1], n[2], -dot(n, p)];
        let mut q = Self::default();
        let mut k = 0;
        for i in 0..4 {
            for j in i..4 {
                q.0[k] = weight * v[i] * v[j];
                k += 1;
            }
        }
        q
    }

    fn add(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }

    fn at(self, p: Point) -> f64 {
        let [x, y, z] = p;
        let q = self.0;
        q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9]
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    cost: f64,
    from: u32,
    to: u32,
    versions: [u64; 2],
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max heap; lower cost and then lower ids win.
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.from.cmp(&self.from))
            .then_with(|| other.to.cmp(&self.to))
            .then_with(|| other.versions.cmp(&self.versions))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

struct Face {
    corners: Corners,
    group: u32,
    alive: bool,
    /// Sorted source triangle ids whose certificates use this face.
    witnessed: Vec<usize>,
}

/// One grid for the immutable source, with a group filter at query time.
struct Source {
    grid: BinGrid,
    /// Normalized f64 geometry; the f32 grid only supplies candidates.
    exact: Vec<[Point; 3]>,
    seen: Vec<bool>,
    visited: Vec<usize>,
}

impl Source {
    fn bin(x: f64) -> i64 {
        (((x + 1.0) / BIN_SIZE).floor() as i64).clamp(0, GRID_RES - 1)
    }

    fn nearest(
        &mut self,
        point: Point,
        group: u32,
        groups: &[u32],
        bound: f64,
        faces: usize,
        ctl: &mut dyn FnMut(usize) -> bool,
    ) -> Result<Option<(f64, usize)>, ()> {
        for id in self.visited.drain(..) {
            self.seen[id] = false;
        }
        let pb = point.map(Self::bin);
        let mut best = f64::INFINITY;
        let mut nearest = None;
        let mut previous: Option<([i64; 3], [i64; 3])> = None;
        for radius in 0..GRID_RES {
            let lo = pb.map(|v| (v - radius).max(0));
            let hi = pb.map(|v| (v + radius).min(GRID_RES - 1));
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    if !ctl(faces) {
                        return Err(());
                    }
                    for z in lo[2]..=hi[2] {
                        if let Some((old_lo, old_hi)) = previous {
                            let b = [x, y, z];
                            if (0..3).all(|a| b[a] >= old_lo[a] && b[a] <= old_hi[a]) {
                                continue;
                            }
                        }
                        for &id in self.grid.bin_tris([x, y, z]) {
                            let id = id as usize;
                            if groups[id] != group || self.seen[id] {
                                continue;
                            }
                            if !ctl(faces) {
                                return Err(());
                            }
                            self.seen[id] = true;
                            self.visited.push(id);
                            let d = distance_sq(point, self.exact[id]);
                            if d < best {
                                best = d;
                                nearest = Some(id);
                            }
                        }
                    }
                }
            }
            let mut boundary = f64::INFINITY;
            for a in 0..3 {
                if lo[a] > 0 {
                    boundary = boundary.min(point[a] - (-1.0 + lo[a] as f64 * BIN_SIZE));
                }
                if hi[a] < GRID_RES - 1 {
                    boundary = boundary.min((-1.0 + (hi[a] + 1) as f64 * BIN_SIZE) - point[a]);
                }
            }
            let guaranteed = (boundary - BIN_DILATE).max(0.0);
            if best.sqrt() <= guaranteed
                || guaranteed > bound
                || (lo == [0; 3] && hi == [GRID_RES - 1; 3])
            {
                break;
            }
            previous = Some((lo, hi));
        }
        Ok(nearest.map(|id| (best.sqrt(), id)))
    }
}

struct SourceField<'a> {
    source: &'a mut Source,
    group: u32,
    groups: &'a [u32],
    bound: f64,
    faces: usize,
    ctl: &'a mut dyn FnMut(usize) -> bool,
}

impl Field for SourceField<'_> {
    fn nearest(&mut self, p: Point) -> Result<Option<(f64, usize)>, ()> {
        self.source.nearest(p, self.group, self.groups, self.bound, self.faces, self.ctl)
    }

    fn distance(&mut self, p: Point, id: usize) -> f64 {
        distance_sq(p, self.source.exact[id]).sqrt()
    }
}

struct RingField<'a> {
    triangles: &'a [[Point; 3]],
    faces: usize,
    ctl: &'a mut dyn FnMut(usize) -> bool,
}

impl Field for RingField<'_> {
    fn nearest(&mut self, p: Point) -> Result<Option<(f64, usize)>, ()> {
        if !(self.ctl)(self.faces) {
            return Err(());
        }
        Ok(self.triangles.iter().enumerate()
            .map(|(id, &t)| (distance_sq(p, t).sqrt(), id))
            .min_by(|a, b| a.0.total_cmp(&b.0)))
    }

    fn distance(&mut self, p: Point, id: usize) -> f64 {
        distance_sq(p, self.triangles[id]).sqrt()
    }
}

struct State<'a> {
    input: &'a SimplifyInput<'a>,
    positions: Vec<Point>,
    faces: Vec<Face>,
    rings: Vec<BTreeSet<usize>>,
    edges: HashMap<Edge, BTreeSet<usize>>,
    spans: HashMap<Edge, Vec<u32>>,
    /// Sorted live face ids used by each source triangle's certificate.
    witnesses: Vec<Vec<usize>>,
    quadrics: Vec<Quadric>,
    versions: Vec<u64>,
    alive: Vec<bool>,
    count: usize,
}

fn validate(input: &SimplifyInput, levels: &[SimplifyLevel]) -> Result<(Vec<Point>, f64), String> {
    for level in levels {
        if !level.max_error.is_finite() || level.max_error < 0.0 {
            return Err("max_error must be finite and nonnegative".into());
        }
    }
    if input.wedges.len() != input.triangles.len() {
        return Err("wedges must have one entry per triangle".into());
    }
    if input.groups.len() != input.triangles.len() {
        return Err("groups must have one entry per triangle".into());
    }
    for (id, p) in input.positions.iter().enumerate() {
        if !p.iter().all(|v| v.is_finite()) {
            return Err(format!("position {id} is not finite"));
        }
    }
    for (id, uv) in input.wedge_uv.iter().enumerate() {
        if !uv.iter().all(|v| v.is_finite()) {
            return Err(format!("wedge UV {id} is not finite"));
        }
    }
    for (id, tri) in input.triangles.iter().enumerate() {
        if tri.iter().any(|&v| v as usize >= input.positions.len()) {
            return Err(format!("triangle {id} has an out-of-range position index"));
        }
        if input.wedges[id]
            .iter()
            .any(|&w| w as usize >= input.wedge_uv.len())
        {
            return Err(format!("triangle {id} has an out-of-range wedge id"));
        }
        if input.groups[id] as usize >= input.uv_bound.len() {
            return Err(format!("triangle {id} has a group not covered by uv_bound"));
        }
    }
    // Divide before summing to avoid overflowing a large translated part.
    let mut center = [0.0; 3];
    for p in input.positions {
        for a in 0..3 {
            center[a] += p[a] / input.positions.len() as f64;
        }
    }
    // Leave a margin inside the source grid's fixed [-1, 1] domain.
    const GRID_MARGIN: f64 = 0.98;
    let extent = input
        .positions
        .iter()
        .flat_map(|p| sub(*p, center))
        .map(f64::abs)
        .fold(0.0, f64::max);
    let scale = if extent > 0.0 {
        GRID_MARGIN / extent
    } else {
        1.0
    };
    let positions = input
        .positions
        .iter()
        .enumerate()
        .map(|(id, p)| {
            let p = sub(*p, center).map(|v| v * scale);
            if p.iter().all(|&v| v.is_finite() && (v as f32).is_finite()) {
                Ok(p)
            } else {
                Err(format!(
                    "centered position {id} is outside the finite BinGrid range"
                ))
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((positions, scale))
}

impl<'a> State<'a> {
    fn new(
        input: &'a SimplifyInput<'a>,
        positions: Vec<Point>,
        ctl: &mut dyn FnMut(usize) -> bool,
    ) -> Option<Self> {
        let count = input.triangles.len();
        let mut state = Self {
            input,
            positions,
            faces: Vec::with_capacity(count),
            rings: vec![BTreeSet::new(); input.positions.len()],
            edges: HashMap::new(),
            spans: HashMap::new(),
            witnesses: Vec::with_capacity(count),
            quadrics: vec![Quadric::default(); input.positions.len()],
            versions: vec![0; input.positions.len()],
            alive: vec![true; input.positions.len()],
            count,
        };
        for (id, tri) in input.triangles.iter().enumerate() {
            if !ctl(count) {
                return None;
            }
            let group = input.groups[id];
            let corners = std::array::from_fn(|i| (tri[i], input.wedges[id][i]));
            state.faces.push(Face {
                corners,
                group,
                alive: true,
                witnessed: vec![id],
            });
            state.witnesses.push(vec![id]);
            state.attach(id);
            let t = state.geometry(corners);
            let n = normal(t);
            let length = dot(n, n).sqrt();
            if length > 0.0 {
                let q = Quadric::plane(n.map(|v| v / length), t[0], 0.5 * length);
                for &v in tri {
                    state.quadrics[v as usize].add(q);
                }
            }
        }
        // Hash iteration must not determine the order of floating-point sums.
        let mut edges: Vec<_> = state.edges.keys().copied().collect();
        edges.sort_unstable();
        for e in edges {
            if !ctl(count) {
                return None;
            }
            if !state.feature(e) {
                continue;
            }
            state.spans.insert(e, vec![e.0, e.1]);
            let p = state.positions[e.0 as usize];
            let direction = sub(state.positions[e.1 as usize], p);
            let weight = dot(direction, direction);
            for &id in &state.edges[&e] {
                let face_n = normal(state.geometry(state.faces[id].corners));
                let n = cross(direction, face_n);
                let length = dot(n, n).sqrt();
                if length > 0.0 {
                    let q = Quadric::plane(n.map(|v| v / length), p, weight);
                    state.quadrics[e.0 as usize].add(q);
                    state.quadrics[e.1 as usize].add(q);
                }
            }
        }
        Some(state)
    }

    fn geometry(&self, corners: Corners) -> [Point; 3] {
        corners.map(|c| self.positions[c.0 as usize])
    }

    fn attach(&mut self, id: usize) {
        let c = self.faces[id].corners;
        for i in 0..3 {
            self.rings[c[i].0 as usize].insert(id);
            if c[i].0 != c[(i + 1) % 3].0 {
                self.edges
                    .entry(edge(c[i].0, c[(i + 1) % 3].0))
                    .or_default()
                    .insert(id);
            }
        }
    }

    fn detach(&mut self, id: usize) {
        let c = self.faces[id].corners;
        for i in 0..3 {
            self.rings[c[i].0 as usize].remove(&id);
            let e = edge(c[i].0, c[(i + 1) % 3].0);
            if let Some(faces) = self.edges.get_mut(&e) {
                faces.remove(&id);
                if faces.is_empty() {
                    self.edges.remove(&e);
                }
            }
        }
    }

    fn wedge(corners: Corners, vertex: u32) -> Option<u32> {
        corners.iter().find(|c| c.0 == vertex).map(|c| c.1)
    }

    fn feature(&self, e: Edge) -> bool {
        let Some(faces) = self.edges.get(&e) else {
            return false;
        };
        if faces.len() != 2 {
            return true;
        }
        let mut it = faces.iter();
        let a = &self.faces[*it.next().unwrap()];
        let b = &self.faces[*it.next().unwrap()];
        self.feature_between(e, (a.group, a.corners), (b.group, b.corners))
    }

    fn feature_between(&self, e: Edge, a: (u32, Corners), b: (u32, Corners)) -> bool {
        a.0 != b.0
            || ((self.input.uv_bound[a.0 as usize] || self.input.uv_bound[b.0 as usize])
                && (Self::wedge(a.1, e.0) != Self::wedge(b.1, e.0)
                    || Self::wedge(a.1, e.1) != Self::wedge(b.1, e.1)))
    }

    fn neighbours(&self, v: u32) -> BTreeSet<u32> {
        self.rings[v as usize]
            .iter()
            .flat_map(|&id| self.faces[id].corners.map(|c| c.0))
            .filter(|&n| n != v)
            .collect()
    }

    fn curve_neighbours(&self, v: u32) -> Vec<u32> {
        self.neighbours(v)
            .into_iter()
            .filter(|&n| self.feature(edge(v, n)))
            .collect()
    }

    fn enqueue(&self, from: u32, to: u32, heap: &mut BinaryHeap<Candidate>) {
        let mut q = self.quadrics[from as usize];
        q.add(self.quadrics[to as usize]);
        heap.push(Candidate {
            cost: q.at(self.positions[to as usize]),
            from,
            to,
            versions: [self.versions[from as usize], self.versions[to as usize]],
        });
    }
}

struct Collapse {
    ring: Vec<(usize, Corners)>,
    /// Source triangle id and its sorted new witness face ids.
    assignments: Vec<(usize, Vec<usize>)>,
    curve: Option<(Edge, Vec<u32>)>,
    forward: f64,
    backward: f64,
}

/// A source triangle a collapse must re-witness, with the candidate faces
/// (ring faces of its group, then its other witnesses) as they would be.
struct Recheck {
    s: usize,
    ids: Vec<usize>,
    geometry: Vec<[Point; 3]>,
    source_uv: [[f64; 2]; 3],
    area: f64,
    textured: bool,
    candidate_uv: Vec<Option<[[f64; 2]; 3]>>,
}

impl State<'_> {
    /// The source triangles witnessed by `from`'s ring, in source order, each
    /// with its candidates. `None`: a textured one has a non-finite UV area.
    fn rechecks(
        &self,
        from: u32,
        ring: &[(usize, Corners)],
        ctl: &mut dyn FnMut(usize) -> bool,
    ) -> Result<Option<Vec<Recheck>>, ()> {
        let mut recheck = BTreeSet::new();
        for &id in &self.rings[from as usize] {
            for &s in &self.faces[id].witnessed {
                if !ctl(self.count) {
                    return Err(());
                }
                recheck.insert(s);
            }
        }
        let mut rechecks = Vec::with_capacity(recheck.len());
        for s in recheck {
            if !ctl(self.count) {
                return Err(());
            }
            let group = self.input.groups[s];
            let mut ids: Vec<_> = ring.iter()
                .filter_map(|&(id, _)| (self.faces[id].group == group).then_some(id))
                .collect();
            ids.extend(self.witnesses[s].iter().copied()
                .filter(|id| !self.rings[from as usize].contains(id)));
            ids.sort_unstable();
            ids.dedup();
            let candidates: Vec<_> = ids.iter().map(|&id| {
                match ring.binary_search_by_key(&id, |entry| entry.0) {
                    Ok(i) => ring[i].1,
                    Err(_) => self.faces[id].corners,
                }
            }).collect();
            let geometry: Vec<_> = candidates.iter().map(|&c| self.geometry(c)).collect();
            let source_uv = self.input.wedges[s].map(|w| self.input.wedge_uv[w as usize]);
            let area = uv_area(&source_uv);
            let textured = self.input.uv_bound[group as usize] && area != 0.0;
            if textured && !area.is_finite() {
                return Ok(None);
            }
            let candidate_uv = if textured {
                candidates
                    .iter()
                    .map(|corners| {
                        let uv = corners.map(|c| self.input.wedge_uv[c.1 as usize]);
                        (uv_area(&uv) != 0.0).then_some(uv)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            rechecks.push(Recheck {
                s, ids, geometry, source_uv, area, textured, candidate_uv,
            });
        }
        Ok(Some(rechecks))
    }

    fn target_wedge(&self, from: u32, to: u32, wedge: u32, group: u32) -> Option<u32> {
        // Prefer the same chart across the collapsing edge. Where a group has
        // several other wedges at the target, choose its lowest id.
        self.edges[&edge(from, to)]
            .iter()
            .filter_map(|&id| {
                let f = &self.faces[id];
                if f.group == group && Self::wedge(f.corners, from) == Some(wedge) {
                    Self::wedge(f.corners, to)
                } else {
                    None
                }
            })
            .min()
            .or_else(|| {
                self.rings[to as usize]
                    .iter()
                    .filter(|&&id| self.faces[id].group == group)
                    .filter_map(|&id| Self::wedge(self.faces[id].corners, to))
                    .min()
            })
    }

    fn check(
        &self,
        from: u32,
        to: u32,
        source: &mut Source,
        bound: f64,
        ctl: &mut dyn FnMut(usize) -> bool,
    ) -> Result<Option<Collapse>, ()> {
        let e = edge(from, to);
        let Some(edge_faces) = self.edges.get(&e) else {
            return Ok(None);
        };
        let curve = self.curve_neighbours(from);
        if !curve.is_empty() && (curve.len() != 2 || !curve.contains(&to)) {
            return Ok(None);
        }
        if edge_faces.is_empty() || edge_faces.len() > 2 {
            return Ok(None);
        }
        let from_neighbours = self.neighbours(from);
        let to_neighbours = self.neighbours(to);
        let shared: BTreeSet<_> = from_neighbours
            .intersection(&to_neighbours)
            .copied()
            .collect();
        let opposite: BTreeSet<_> = edge_faces
            .iter()
            .flat_map(|&id| self.faces[id].corners.map(|c| c.0))
            .filter(|&v| v != from && v != to)
            .collect();
        if opposite.len() != edge_faces.len() || shared != opposite {
            return Ok(None);
        }

        let mut result = Collapse {
            ring: Vec::new(),
            assignments: Vec::new(),
            curve: None,
            forward: 0.0,
            backward: 0.0,
        };
        let mut triangles = BTreeSet::new();
        for &id in self.rings[from as usize].union(&self.rings[to as usize]) {
            if !ctl(self.count) {
                return Err(());
            }
            if edge_faces.contains(&id) {
                continue;
            }
            let old = &self.faces[id];
            let mut corners = old.corners;
            let mut changed = false;
            for c in &mut corners {
                if c.0 == from {
                    let Some(wedge) = self.target_wedge(from, to, c.1, old.group) else {
                        return Ok(None);
                    };
                    *c = (to, wedge);
                    changed = true;
                }
            }
            let mut key = corners.map(|c| c.0);
            key.sort_unstable();
            // The vertex-only link test also needs this simplex check (e.g.
            // collapsing a tetrahedron edge would otherwise make two copies).
            if key[0] == key[1] || key[1] == key[2] || !triangles.insert(key) {
                return Ok(None);
            }
            if changed {
                let t = self.geometry(corners);
                let n = normal(t);
                let area_sq = dot(n, n);
                let orientation = dot(normal(self.geometry(old.corners)), n);
                if !area_sq.is_finite()
                    || sliver(t, n)
                    || !orientation.is_finite()
                    || orientation <= 0.0
                {
                    return Ok(None);
                }
            }
            result.ring.push((id, corners));
        }

        // A textured ring re-witnesses its source by UV, the cheapest
        // rejection: those checks run before any spatial query. An
        // untextured ring keeps the point samples first, so their rejections
        // never build the recheck lists.
        let textured_ring = self.rings[from as usize].iter()
            .any(|&id| self.input.uv_bound[self.faces[id].group as usize]);
        let mut rechecks = Vec::new();
        if textured_ring {
            let Some(built) = self.rechecks(from, &result.ring, ctl)? else {
                return Ok(None);
            };
            rechecks = built;
            result.assignments = rechecks.iter().map(|r| (r.s, Vec::new())).collect();
        }

        // A source corner/centroid strictly inside a candidate is a vertex/interior point of their positive-area UV piece.
        // Deviation is the norm of an affine map, hence convex: its maximum is at a piece vertex, so the exact check rejects too.
        for r in rechecks.iter().filter(|r| r.textured) {
            let source_uv = r.source_uv;
            let centroid = std::array::from_fn(|a| {
                (source_uv[0][a] + source_uv[1][a] + source_uv[2][a]) / 3.0
            });
            for q in [source_uv[0], source_uv[1], source_uv[2], centroid] {
                if !ctl(self.count) {
                    return Err(());
                }
                let Some(source_weights) = uv_weights(q, source_uv) else {
                    continue;
                };
                let source_point = mix(source.exact[r.s], source_weights);
                for (i, uv) in r.candidate_uv.iter().enumerate() {
                    let Some(uv) = *uv else {
                        continue;
                    };
                    let Some(weights) = uv_weights(q, uv) else {
                        continue;
                    };
                    if !weights.iter().all(|&w| w > 1e-9) {
                        continue;
                    }
                    let delta = sub(source_point, mix(r.geometry[i], weights));
                    let d = dot(delta, delta).sqrt();
                    if !(d <= bound) {
                        return Ok(None);
                    }
                }
            }
        }

        let mut scratch = Vec::new();
        for (index, r) in rechecks.iter().enumerate() {
            if !r.textured {
                continue;
            }
            scratch.clear();
            let mut covered = 0.0;
            let mut deviation: f64 = 0.0;
            for (i, uv) in r.candidate_uv.iter().enumerate() {
                if !ctl(self.count) {
                    return Err(());
                }
                let Some(uv) = *uv else {
                    continue;
                };
                let piece = uv_overlap(r.source_uv, uv);
                let piece_area = uv_area(&piece);
                if piece_area <= 0.0 {
                    continue;
                }
                for q in piece {
                    let (Some(source_weights), Some(weights)) =
                        (uv_weights(q, r.source_uv), uv_weights(q, uv)) else {
                            return Ok(None);
                        };
                    let delta = sub(mix(source.exact[r.s], source_weights), mix(r.geometry[i], weights));
                    let d = dot(delta, delta).sqrt();
                    if !(d <= bound) {
                        return Ok(None);
                    }
                    deviation = deviation.max(d);
                }
                covered += piece_area;
                scratch.push(i);
            }
            // Less area leaves source UV uncovered; more means the
            // candidate faces overlap in UV. Refuse either case.
            if !((covered - r.area).abs() <= 1e-9 * r.area) {
                return Ok(None);
            }
            result.backward = result.backward.max(deviation);
            result.assignments[index].1 = scratch.iter().map(|&i| r.ids[i]).collect();
        }

        // These points only reject; the forward whole-face certificate runs last.
        for &(id, corners) in &result.ring {
            if !self.rings[from as usize].contains(&id) {
                continue;
            }
            let t = self.geometry(corners);
            for weights in [
                [0.5, 0.5, 0.0],
                [0.0, 0.5, 0.5],
                [0.5, 0.0, 0.5],
                [1.0 / 3.0; 3],
            ] {
                let Some((d, _)) = source.nearest(
                    mix(t, weights), self.faces[id].group, self.input.groups,
                    bound, self.count, ctl,
                )? else {
                    return Ok(None);
                };
                if !(d <= bound) {
                    return Ok(None);
                }
            }
        }

        if !textured_ring {
            let Some(built) = self.rechecks(from, &result.ring, ctl)? else {
                return Ok(None);
            };
            rechecks = built;
            result.assignments = rechecks.iter().map(|r| (r.s, Vec::new())).collect();
        }
        for (index, r) in rechecks.iter().enumerate() {
            if r.textured {
                continue;
            }
            let Some(u) = certify(source.exact[r.s], bound, &mut RingField {
                triangles: &r.geometry, faces: self.count, ctl,
            }, &mut scratch)? else {
                return Ok(None);
            };
            result.backward = result.backward.max(u);
            result.assignments[index].1 = scratch.iter().map(|&i| r.ids[i]).collect();
        }

        if !curve.is_empty() {
            let other = if curve[0] == to { curve[1] } else { curve[0] };
            let new_edge = edge(to, other);
            let adjacent: Vec<_> = result
                .ring
                .iter()
                .filter_map(|&(id, corners)| {
                    (Self::wedge(corners, to).is_some() && Self::wedge(corners, other).is_some())
                        .then_some((self.faces[id].group, corners))
                })
                .collect();
            if adjacent.is_empty()
                || (adjacent.len() == 2
                    && !self.feature_between(new_edge, adjacent[0], adjacent[1]))
            {
                return Ok(None);
            }
            let mut span = BTreeSet::new();
            for key in [e, edge(from, other), new_edge] {
                if let Some(vertices) = self.spans.get(&key) {
                    span.extend(vertices.iter().copied());
                }
            }
            span.insert(from);
            for &v in &span {
                if !ctl(self.count) {
                    return Err(());
                }
                let d = segment_distance_sq(
                    self.positions[v as usize],
                    self.positions[to as usize],
                    self.positions[other as usize],
                )
                .sqrt();
                if !d.is_finite() || d > bound {
                    return Ok(None);
                }
                result.backward = result.backward.max(d);
            }
            result.curve = Some((new_edge, span.into_iter().collect()));
        }
        for &(id, corners) in &result.ring {
            if !self.rings[from as usize].contains(&id) {
                continue;
            }
            let Some(u) = certify(self.geometry(corners), bound, &mut SourceField {
                source, group: self.faces[id].group, groups: self.input.groups,
                bound, faces: self.count, ctl,
            }, &mut scratch)? else {
                return Ok(None);
            };
            result.forward = result.forward.max(u);
        }
        Ok(Some(result))
    }

    fn commit(&mut self, from: u32, to: u32, collapse: Collapse, heap: &mut BinaryHeap<Candidate>) {
        let old_ring: Vec<_> = self.rings[from as usize].iter().copied().collect();
        let mut affected = BTreeSet::new();
        let mut touched_edges = BTreeSet::new();
        for &id in &old_ring {
            let c = self.faces[id].corners;
            affected.extend(c.map(|c| c.0));
            for i in 0..3 {
                touched_edges.insert(edge(c[i].0, c[(i + 1) % 3].0));
            }
            self.detach(id);
        }
        for (id, corners) in &collapse.ring {
            affected.extend(corners.map(|c| c.0));
            if old_ring.binary_search(id).is_ok() {
                self.faces[*id].corners = *corners;
                self.attach(*id);
                for i in 0..3 {
                    touched_edges.insert(edge(corners[i].0, corners[(i + 1) % 3].0));
                }
            }
        }
        for id in old_ring {
            if collapse
                .ring
                .binary_search_by_key(&id, |entry| entry.0)
                .is_err()
            {
                self.faces[id].alive = false;
                self.count -= 1;
            }
        }

        for (s, faces) in collapse.assignments {
            for &id in &self.witnesses[s] {
                affected.extend(self.faces[id].corners.map(|c| c.0));
                self.faces[id].witnessed.retain(|&source| source != s);
            }
            self.witnesses[s] = faces;
            for &id in &self.witnesses[s] {
                affected.extend(self.faces[id].corners.map(|c| c.0));
                let witnessed = &mut self.faces[id].witnessed;
                if let Err(i) = witnessed.binary_search(&s) {
                    witnessed.insert(i, s);
                }
            }
        }
        for e in touched_edges {
            if self.feature(e) {
                self.spans.entry(e).or_insert_with(|| vec![e.0, e.1]);
            } else {
                self.spans.remove(&e);
            }
        }
        if let Some((e, span)) = collapse.curve {
            self.spans.insert(e, span);
        }
        let q = self.quadrics[from as usize];
        self.quadrics[to as usize].add(q);
        self.alive[from as usize] = false;
        for &v in &affected {
            self.versions[v as usize] += 1;
        }
        // Endpoints of changed faces, witness lists, and the new target ring
        // cover every local dependency of check(). Rejections are requeued
        // after such a dependency changes, never immediately on rejection.
        // The rising level bound is another dependency: each level boundary
        // requeues every live directed edge, including previously rejected ones.
        let mut candidates = BTreeSet::new();
        for v in affected {
            if self.alive[v as usize] {
                for n in self.neighbours(v) {
                    candidates.insert((v, n));
                    candidates.insert((n, v));
                }
            }
        }
        for (a, b) in candidates {
            self.enqueue(a, b, heap);
        }
    }
}

/// Simplify one welded part progressively, returning one snapshot per level.
/// Each error certifies the two-sided Hausdorff distance to the immutable input
/// surface per group, and texture deviation for textured groups. The subdivision
/// floor refuses an uncertified collapse, never errors. The effective bound is the
/// minimum max_error in that level and all later levels, so bounds never decrease
/// and each snapshot meets its own requested bound even when authored bounds shrink.
/// `ctl` may cancel; invalid inputs are always errors, even when no reduction
/// or no levels were requested.
pub fn simplify_levels(
    input: &SimplifyInput,
    levels: &[SimplifyLevel],
    ctl: &mut dyn FnMut(usize) -> bool,
) -> Result<Option<Vec<SimplifyOutput>>, String> {
    let (positions, scale) = validate(input, levels)?;
    if !ctl(input.triangles.len()) {
        return Ok(None);
    }
    if levels.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let mut bounds: Vec<_> = levels.iter().map(|level| level.max_error).collect();
    for i in (0..bounds.len() - 1).rev() {
        bounds[i] = bounds[i].min(bounds[i + 1]);
    }
    let Some(mut state) = State::new(input, positions, ctl) else {
        return Ok(None);
    };
    let mut exact = Vec::with_capacity(input.triangles.len());
    for tri in input.triangles {
        if !ctl(state.count) {
            return Ok(None);
        }
        exact.push(tri.map(|v| state.positions[v as usize]));
    }
    let grid = {
        let triangles: Vec<_> = exact.iter().map(|t| t.map(|p| p.map(|v| v as f32))).collect();
        BinGrid::build(&triangles)
    };
    let mut source = Source {
        grid,
        seen: vec![false; exact.len()],
        visited: Vec::new(),
        exact,
    };
    let mut heap = BinaryHeap::new();
    let mut outputs = Vec::with_capacity(levels.len());
    let mut max_forward: f64 = 0.0;
    let mut max_backward: f64 = 0.0;
    for (level, bound) in levels.iter().zip(bounds) {
        let bound = bound * scale;
        // A larger bound can admit an edge rejected at the preceding level.
        // Use the same sorted order for the initial heap and every rebuild.
        heap.clear();
        let mut edges: Vec<_> = state.edges.keys().copied().collect();
        edges.sort_unstable();
        for (a, b) in edges {
            if !ctl(state.count) {
                return Ok(None);
            }
            state.enqueue(a, b, &mut heap);
            state.enqueue(b, a, &mut heap);
        }
        let mut stop = SimplifyStop::Target;
        while state.count > level.target_faces {
            if !ctl(state.count) {
                return Ok(None);
            }
            let Some(candidate) = heap.pop() else {
                stop = SimplifyStop::NoCandidate;
                break;
            };
            let Candidate {
                from,
                to,
                versions,
                cost,
            } = candidate;
            if !state.alive[from as usize]
                || !state.alive[to as usize]
                || versions != [state.versions[from as usize], state.versions[to as usize]]
                || !cost.is_finite()
            {
                continue;
            }
            match state.check(from, to, &mut source, bound, ctl) {
                Err(()) => return Ok(None),
                Ok(None) => (),
                Ok(Some(collapse)) => {
                    max_forward = max_forward.max(collapse.forward);
                    max_backward = max_backward.max(collapse.backward);
                    state.commit(from, to, collapse, &mut heap);
                }
            }
        }
        if !ctl(state.count) {
            return Ok(None);
        }
        let mut output = SimplifyOutput {
            triangles: Vec::with_capacity(state.count),
            groups: Vec::with_capacity(state.count),
            report: SimplifyReport {
                faces: state.count,
                max_forward: max_forward / scale,
                max_backward: max_backward / scale,
                stop,
            },
        };
        for face in &state.faces {
            if face.alive {
                output.triangles.push(face.corners);
                output.groups.push(face.group);
            }
        }
        outputs.push(output);
    }
    Ok(Some(outputs))
}

#[cfg(test)]
mod tests {
    use super::{certify, simplify_levels, Field, SimplifyInput, SimplifyLevel, SimplifyStop};
    use crate::post::audit_mesh_topology;
    use std::collections::BTreeMap;
    use std::f64::consts::{PI, TAU};

    struct Fixture {
        name: &'static str,
        positions: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
        wedges: Vec<[u32; 3]>,
        uv: Vec<[f64; 2]>,
        groups: Vec<u32>,
        uv_bound: Vec<bool>,
        closed: bool,
        target: usize,
    }

    impl Fixture {
        fn new(
            name: &'static str,
            positions: Vec<[f64; 3]>,
            triangles: Vec<[u32; 3]>,
            closed: bool,
            target: usize,
        ) -> Self {
            Self {
                name,
                uv: vec![[0.0; 2]; positions.len()],
                groups: vec![0; triangles.len()],
                wedges: triangles.clone(),
                positions,
                triangles,
                uv_bound: vec![false],
                closed,
                target,
            }
        }

        fn input(&self) -> SimplifyInput<'_> {
            SimplifyInput {
                positions: &self.positions,
                triangles: &self.triangles,
                wedges: &self.wedges,
                wedge_uv: &self.uv,
                groups: &self.groups,
                uv_bound: &self.uv_bound,
            }
        }
    }

    fn torus() -> Fixture {
        let (nu, nv) = (48, 12);
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for i in 0..nu {
            let u = TAU * i as f64 / nu as f64;
            for j in 0..nv {
                let v = TAU * j as f64 / nv as f64;
                let r = 0.24 + 0.105 * v.cos();
                positions.push([r * u.cos(), 0.105 * v.sin(), r * u.sin()]);
                let a = (i * nv + j) as u32;
                let b = (((i + 1) % nu) * nv + j) as u32;
                let c = (((i + 1) % nu) * nv + (j + 1) % nv) as u32;
                let d = (i * nv + (j + 1) % nv) as u32;
                triangles.extend([[a, d, c], [a, c, b]]);
            }
        }
        Fixture::new("torus", positions, triangles, true, 120)
    }

    fn rounded_box() -> Fixture {
        let half = [0.15, 0.05, 0.03];
        let radius = 0.002;
        let mut ids = BTreeMap::new();
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        let mut wedges = Vec::new();
        let mut uv = Vec::new();
        for axis in 0..3 {
            for sign in [-1, 1] {
                let mut face = [[(0, 0); 5]; 5];
                for (u, row) in face.iter_mut().enumerate() {
                    for (v, corner) in row.iter_mut().enumerate() {
                        let mut key = [0; 3];
                        key[axis] = sign * 2;
                        key[(axis + 1) % 3] = u as i32 - 2;
                        key[(axis + 2) % 3] = v as i32 - 2;
                        let id = *ids.entry(key).or_insert_with(|| {
                            let raw: [f64; 3] = std::array::from_fn(|a| {
                                [-half[a], -half[a] + radius, 0.0, half[a] - radius, half[a]]
                                    [(key[a] + 2) as usize]
                            });
                            let core: [f64; 3] = std::array::from_fn(|a| {
                                raw[a].clamp(-half[a] + radius, half[a] - radius)
                            });
                            let delta: [f64; 3] = std::array::from_fn(|a| raw[a] - core[a]);
                            let length = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
                            let p = std::array::from_fn(|a| core[a] + radius * delta[a] / length);
                            let id = positions.len() as u32;
                            positions.push(p);
                            id
                        });
                        *corner = (id, uv.len() as u32);
                        // Six separate face charts: every cube edge is a UV
                        // seam, but none is a feature on this untextured part.
                        uv.push([u as f64 / 4.0, v as f64 / 4.0]);
                    }
                }
                for u in 0..4 {
                    for v in 0..4 {
                        let a = face[u][v];
                        let b = face[u + 1][v];
                        let c = face[u + 1][v + 1];
                        let d = face[u][v + 1];
                        for mut t in [[a, b, c], [a, c, d]] {
                            if sign < 0 {
                                t.swap(1, 2);
                            }
                            triangles.push(t.map(|c| c.0));
                            wedges.push(t.map(|c| c.1));
                        }
                    }
                }
            }
        }
        let mut f = Fixture::new("rounded box", positions, triangles, true, 12);
        f.wedges = wedges;
        f.uv = uv;
        f
    }

    fn sphere() -> Fixture {
        let (nu, nv) = (32, 16);
        let mut positions = vec![[0.0, 0.1, 0.0]];
        for j in 1..nv {
            let v = PI * j as f64 / nv as f64;
            for i in 0..nu {
                let u = TAU * i as f64 / nu as f64;
                positions.push([
                    0.1 * v.sin() * u.cos(),
                    0.1 * v.cos(),
                    0.1 * v.sin() * u.sin(),
                ]);
            }
        }
        let bottom = positions.len() as u32;
        positions.push([0.0, -0.1, 0.0]);
        let mut triangles = Vec::new();
        let id = |i: usize, j: usize| (1 + (j - 1) * nu + i % nu) as u32;
        for i in 0..nu {
            triangles.push([0, id(i + 1, 1), id(i, 1)]);
            for j in 1..nv - 1 {
                let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
                triangles.extend([[a, b, c], [a, c, d]]);
            }
            triangles.push([id(i, nv - 1), id(i + 1, nv - 1), bottom]);
        }
        Fixture::new("UV sphere", positions, triangles, true, 48)
    }

    fn washer() -> Fixture {
        let n = 32;
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for i in 0..n {
            let u = TAU * i as f64 / n as f64;
            for (j, (radius, y)) in [(0.12, 0.002), (0.10, 0.002), (0.10, -0.002), (0.12, -0.002)]
                .into_iter()
                .enumerate()
            {
                positions.push([radius * u.cos(), y, radius * u.sin()]);
                let (a, b, c, d) = (
                    (i * 4 + j) as u32,
                    (((i + 1) % n) * 4 + j) as u32,
                    (((i + 1) % n) * 4 + (j + 1) % 4) as u32,
                    (i * 4 + (j + 1) % 4) as u32,
                );
                triangles.extend([[a, d, c], [a, c, b]]);
            }
        }
        Fixture::new("washer", positions, triangles, true, 64)
    }

    fn patch(textured: bool) -> Fixture {
        let n = 8;
        let mut positions = Vec::new();
        let mut uv = Vec::new();
        let mut triangles = Vec::new();
        let mut groups = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let (u, v) = (x as f64 / n as f64, y as f64 / n as f64);
                positions.push(if textured {
                    [
                        0.24 * (u - 0.5),
                        0.24 * (v - 0.5),
                        0.08 * (u * (1.0 - u) + v * (1.0 - v)),
                    ]
                } else {
                    [0.4 * (u - 0.5), 0.4 * (v - 0.5), 0.0]
                });
                uv.push([u, v]);
                if x < n && y < n {
                    let a = (y * (n + 1) + x) as u32;
                    let b = a + 1;
                    let d = a + (n + 1) as u32;
                    let c = d + 1;
                    triangles.extend([[a, b, c], [a, c, d]]);
                    let group = u32::from(!textured && x >= n / 2);
                    groups.extend([group; 2]);
                }
            }
        }
        let mut f = Fixture::new(
            if textured {
                "textured chart"
            } else {
                "two-material plate"
            },
            positions,
            triangles,
            false,
            8,
        );
        f.uv = uv;
        f.groups = groups;
        f.uv_bound = if textured { vec![true] } else { vec![false; 2] };
        f
    }

    fn saddle() -> Fixture {
        let n = 6;
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (i as f64 / n as f64 - 0.5, j as f64 / n as f64 - 0.5);
                positions.push([x, y, 0.3 * (x * x - y * y)]);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let a = j * (n + 1) + i;
                let b = a + 1;
                let d = a + n + 1;
                let c = d + 1;
                // Split along the valley: the diagonal with the lower midpoint.
                let ac = (positions[a][2] + positions[c][2]) * 0.5;
                let bd = (positions[b][2] + positions[d][2]) * 0.5;
                let quad = if ac <= bd { [[a, b, c], [a, c, d]] }
                    else { [[a, b, d], [b, c, d]] };
                triangles.extend(quad.map(|t| t.map(|v| v as u32)));
            }
        }
        Fixture::new("saddle", positions, triangles, false, 0)
    }

    // A flat ring around an inner diamond. Every inner vertex collapses at
    // zero cost, so ids order them: 0 into 1 comes first and would join 1, 4
    // and 2 (a cut line's collinear vertices, 2 sits 1e-12 off the line) into
    // a face mesh validation refuses; 0 into 2 reduces cleanly.
    fn sliver_fan() -> Fixture {
        let positions = vec![
            [0.0, 0.3, 0.0], [1.0, 0.0, 0.0], [0.0, -1e-12, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0],
            [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [-2.0, 0.0, 0.0], [0.0, -2.0, 0.0],
        ];
        let mut triangles = vec![[0, 2, 1], [0, 1, 3], [0, 3, 4], [0, 4, 2], [2, 5, 1], [2, 4, 5]];
        for (inner, outer) in [(1, 6), (3, 7), (4, 8), (5, 9)] {
            let (next_inner, next_outer) = match inner { 1 => (3, 7), 3 => (4, 8), 4 => (5, 9), _ => (1, 6) };
            triangles.extend([[inner, outer, next_outer], [inner, next_outer, next_inner]]);
        }
        Fixture::new("sliver fan", positions, triangles, false, 12)
    }

    // The oracle deliberately uses a different point/triangle algorithm:
    // Voronoi regions, with precomputed edge dot products. It shares neither
    // the simplifier's distance/UV routines nor its grid or assignments.
    fn minus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|i| a[i] - b[i])
    }

    fn inner(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    struct OracleTriangle {
        p: [[f64; 3]; 3],
        ab: [f64; 3],
        ac: [f64; 3],
        aa: f64,
        ab_ac: f64,
        cc: f64,
    }

    impl OracleTriangle {
        fn new(p: [[f64; 3]; 3]) -> Self {
            let ab = minus(p[1], p[0]);
            let ac = minus(p[2], p[0]);
            Self {
                p,
                ab,
                ac,
                aa: inner(ab, ab),
                ab_ac: inner(ab, ac),
                cc: inner(ac, ac),
            }
        }

        fn point(&self, weights: [f64; 3]) -> [f64; 3] {
            std::array::from_fn(|a| {
                self.p[0][a] * weights[0] + self.p[1][a] * weights[1] + self.p[2][a] * weights[2]
            })
        }

        fn distance_sq(&self, p: [f64; 3]) -> f64 {
            let ap = minus(p, self.p[0]);
            let d1 = inner(self.ab, ap);
            let d2 = inner(self.ac, ap);
            let (v, w);
            if d1 <= 0.0 && d2 <= 0.0 {
                return inner(ap, ap);
            }
            let d3 = d1 - self.aa;
            let d4 = d2 - self.ab_ac;
            if d3 >= 0.0 && d4 <= d3 {
                let bp = minus(p, self.p[1]);
                return inner(bp, bp);
            }
            let vc = d1 * d4 - d3 * d2;
            if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
                v = d1 / (d1 - d3);
                w = 0.0;
            } else {
                let d5 = d1 - self.ab_ac;
                let d6 = d2 - self.cc;
                if d6 >= 0.0 && d5 <= d6 {
                    let cp = minus(p, self.p[2]);
                    return inner(cp, cp);
                }
                let vb = d5 * d2 - d1 * d6;
                if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
                    v = 0.0;
                    w = d2 / (d2 - d6);
                } else {
                    let va = d3 * d6 - d5 * d4;
                    if va <= 0.0 && d4 >= d3 && d5 >= d6 {
                        w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
                        v = 1.0 - w;
                    } else {
                        let inv = 1.0 / (va + vb + vc);
                        v = vb * inv;
                        w = vc * inv;
                    }
                }
            }
            let delta = std::array::from_fn(|a| ap[a] - v * self.ab[a] - w * self.ac[a]);
            inner(delta, delta)
        }
    }

    /// The triangles of `to` whose bounding box is within `reach` of `from`'s
    /// on every axis: every triangle that can hold a point within `reach` of a
    /// point of `from`. Restricting the nearest search to them is exact for any
    /// distance up to `reach`, and can only raise a distance beyond it, so
    /// checks of the form `distance <= bound <= reach` keep their strength.
    fn within_reach(from: &OracleTriangle, to: &[OracleTriangle], reach: f64) -> Vec<usize> {
        let bounds = |t: &OracleTriangle| {
            let lo: [f64; 3] = std::array::from_fn(|a| t.p[0][a].min(t.p[1][a]).min(t.p[2][a]));
            let hi: [f64; 3] = std::array::from_fn(|a| t.p[0][a].max(t.p[1][a]).max(t.p[2][a]));
            (lo, hi)
        };
        let (lo, hi) = bounds(from);
        (0..to.len())
            .filter(|&j| {
                let (tlo, thi) = bounds(&to[j]);
                (0..3).all(|a| tlo[a] - hi[a] <= reach && lo[a] - thi[a] <= reach)
            })
            .collect()
    }

    fn dense_distance(from: &[OracleTriangle], to: &[OracleTriangle], weights: &[[f64; 3]], reach: f64) -> f64 {
        let mut maximum: f64 = 0.0;
        for triangle in from {
            // Every triangle within reach is tested, without the simplifier's
            // grid, a distance threshold, or an early exit at a small distance.
            let near = within_reach(triangle, to, reach);
            for &w in weights {
                let p = triangle.point(w);
                let mut nearest = f64::INFINITY;
                for &j in &near {
                    nearest = nearest.min(to[j].distance_sq(p));
                }
                maximum = maximum.max(nearest);
            }
        }
        maximum.sqrt()
    }

    fn oracle_uv_weights(p: [f64; 2], t: [[f64; 2]; 3]) -> Option<[f64; 3]> {
        let a = [t[1][0] - t[0][0], t[1][1] - t[0][1]];
        let b = [t[2][0] - t[0][0], t[2][1] - t[0][1]];
        let q = [p[0] - t[0][0], p[1] - t[0][1]];
        let dot = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1];
        let (aa, ab, bb, aq, bq) = (dot(a, a), dot(a, b), dot(b, b), dot(a, q), dot(b, q));
        let det = aa * bb - ab * ab;
        if det <= 0.0 {
            return None;
        }
        let v = (bb * aq - ab * bq) / det;
        let w = (aa * bq - ab * aq) / det;
        let weights = [1.0 - v - w, v, w];
        weights.iter().all(|&v| v >= -1e-12).then_some(weights)
    }

    fn texture_distance(
        from: &[OracleTriangle],
        from_uv: &[[[f64; 2]; 3]],
        to: &[OracleTriangle],
        to_uv: &[[[f64; 2]; 3]],
        weights: &[[f64; 3]],
        reach: f64,
    ) -> f64 {
        let mut maximum: f64 = 0.0;
        for (tri, uv) in from.iter().zip(from_uv) {
            // A deviation is at least the distance to that face: the reach
            // filter is exact here too.
            let near = within_reach(tri, to, reach);
            for &w in weights {
                let p = tri.point(w);
                let sample_uv =
                    std::array::from_fn(|a| uv[0][a] * w[0] + uv[1][a] * w[1] + uv[2][a] * w[2]);
                let mut nearest = f64::INFINITY;
                for &j in &near {
                    if let Some(bary) = oracle_uv_weights(sample_uv, to_uv[j]) {
                        let delta = minus(p, to[j].point(bary));
                        nearest = nearest.min(inner(delta, delta));
                    }
                }
                maximum = maximum.max(nearest);
            }
        }
        maximum.sqrt()
    }

    /// Brute force over a fixed set with the oracle's own point-triangle
    /// distance, counting the nearest-triangle queries.
    struct Brute {
        set: Vec<OracleTriangle>,
        queries: usize,
    }

    impl Field for Brute {
        fn nearest(&mut self, p: [f64; 3]) -> Result<Option<(f64, usize)>, ()> {
            self.queries += 1;
            Ok(self
                .set
                .iter()
                .enumerate()
                .map(|(j, t)| (t.distance_sq(p).sqrt(), j))
                .min_by(|a, b| a.0.total_cmp(&b.0)))
        }

        fn distance(&mut self, p: [f64; 3], id: usize) -> f64 {
            self.set[id].distance_sq(p).sqrt()
        }
    }

    fn brute(set: &[[[f64; 3]; 3]]) -> Brute {
        Brute {
            set: set.iter().map(|&t| OracleTriangle::new(t)).collect(),
            queries: 0,
        }
    }

    /// The largest distance to `set` over an `n`-subdivision lattice of `t`,
    /// and an upper bound of it over all of `t`: the distance is 1-Lipschitz,
    /// and every point of a lattice cell is within the cell's longest edge
    /// over sqrt(3) of one of its corners.
    fn lattice_bounds(t: [[f64; 3]; 3], set: &[[[f64; 3]; 3]], n: usize) -> (f64, f64) {
        let triangle = OracleTriangle::new(t);
        let set: Vec<_> = set.iter().map(|&s| OracleTriangle::new(s)).collect();
        let mut low: f64 = 0.0;
        for i in 0..=n {
            for j in 0..=n - i {
                let p = triangle.point([
                    (n - i - j) as f64 / n as f64,
                    i as f64 / n as f64,
                    j as f64 / n as f64,
                ]);
                let d = set
                    .iter()
                    .map(|s| s.distance_sq(p))
                    .fold(f64::INFINITY, f64::min)
                    .sqrt();
                low = low.max(d);
            }
        }
        let longest = [minus(t[1], t[0]), minus(t[2], t[1]), minus(t[0], t[2])]
            .map(|e| inner(e, e).sqrt())
            .into_iter()
            .fold(0.0, f64::max);
        (low, low + longest / n as f64 / 3f64.sqrt())
    }

    #[test]
    fn certify_bounds_every_point_of_the_triangle() {
        let mut witnesses = Vec::new();

        // A flat triangle over a plate with a V groove. The groove line
        // x = 0.2 sqrt(2) is off every dyadic point of the triangle, so no
        // cell corner lies on it, and the edge midpoints and the centroid
        // (the early-reject points) all lie on the plate.
        let (g, w, h) = (0.2 * 2f64.sqrt(), 0.05, 0.05);
        let strip = |x0: f64, z0: f64, x1: f64, z1: f64| {
            [
                [[x0, -1.0, z0], [x1, -1.0, z1], [x1, 1.0, z1]],
                [[x0, -1.0, z0], [x1, 1.0, z1], [x0, 1.0, z0]],
            ]
        };
        let groove: Vec<_> = [
            strip(-1.0, 0.0, g - w, 0.0),
            strip(g - w, 0.0, g, -h),
            strip(g, -h, g + w, 0.0),
            strip(g + w, 0.0, 1.0, 0.0),
        ]
        .into_iter()
        .flatten()
        .collect();
        let flat = [[-0.8, -0.8, 0.0], [0.8, -0.8, 0.0], [0.0, 0.8, 0.0]];
        // The distance from the groove line to a wall: h / sqrt(1 + slope^2).
        let depth = h / (1.0 + (h / w).powi(2)).sqrt();
        let mut field = brute(&groove);
        for weights in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.5, 0.5, 0.0],
            [0.0, 0.5, 0.5],
            [0.5, 0.0, 0.5],
            [1.0 / 3.0; 3],
        ] {
            let p = OracleTriangle::new(flat).point(weights);
            let (d, _) = field.nearest(p).unwrap().unwrap();
            assert!(d < 1e-12, "V groove: the sample at {weights:?} reads {d}, not 0");
        }
        assert_eq!(
            certify(flat, 0.99 * depth, &mut brute(&groove), &mut witnesses),
            Ok(None),
            "V groove: a bound under the groove's depth {depth} must be refused"
        );
        let bound = 1.1 * depth;
        let u = certify(flat, bound, &mut brute(&groove), &mut witnesses)
            .unwrap()
            .unwrap_or_else(|| panic!("V groove: a bound over the depth {depth} must be certified"));
        assert!(
            depth - 1e-12 <= u && u <= bound,
            "V groove: the certified {u} must lie in [{depth}, {bound}]"
        );
        assert!(
            !witnesses.is_empty() && witnesses.iter().all(|&j| j < groove.len()),
            "V groove: witnesses {witnesses:?} are ids of the set"
        );

        // A twisted quad: the source triangle lies on the valley diagonal a-c,
        // the reduced faces on the other diagonal. All of its vertices lie on
        // the reduced faces; the middle of a-c is about half the twist away.
        let tau = 0.2;
        let (a, b, c, d) = (
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, tau],
            [0.0, 1.0, 0.0],
        );
        let source = [a, b, c];
        let reduced = [[a, b, d], [b, c, d]];
        for p in source {
            let (d, _) = brute(&reduced).nearest(p).unwrap().unwrap();
            assert!(d < 1e-12, "twisted quad: vertex {p:?} reads {d}, not 0");
        }
        let (low, high) = lattice_bounds(source, &reduced, 300);
        assert!(low > 0.4 * tau, "twisted quad: the lattice finds the twist ({low})");
        assert_eq!(
            certify(source, 0.99 * low, &mut brute(&reduced), &mut witnesses),
            Ok(None),
            "twisted quad: a bound under the distance {low} must be refused"
        );
        let bound = 1.1 * high;
        let u = certify(source, bound, &mut brute(&reduced), &mut witnesses)
            .unwrap()
            .unwrap_or_else(|| panic!("twisted quad: a bound over the distance {high} must be certified"));
        assert!(
            low <= u && u <= bound,
            "twisted quad: the certified {u} must lie in [{low}, {bound}]"
        );
        assert_eq!(witnesses, [0, 1], "twisted quad: both reduced faces bound the source");

        // A triangle spanning coplanar source triangles: the work follows the
        // source edges under it (linear in size over bound), not its area
        // (quadratic), and the result stays within the bound.
        let square = [
            [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0]],
            [[-1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
        ];
        let span = [[-0.8, -0.7, 0.0], [0.9, -0.6, 0.0], [-0.5, 0.85, 0.0]];
        let mut queries = [0; 2];
        for (k, bound) in [2e-3, 1e-3].into_iter().enumerate() {
            let mut field = brute(&square);
            let u = certify(span, bound, &mut field, &mut witnesses)
                .unwrap()
                .unwrap_or_else(|| panic!("coplanar span: certified at {bound}"));
            assert!(u <= bound, "coplanar span: certified {u} over {bound}");
            queries[k] = field.queries;
        }
        assert!(
            queries[1] <= 3 * queries[0],
            "coplanar span: halving the bound took {} queries after {}; the work must grow \
             with the source edges (x2), not with the area (x4)",
            queries[1],
            queries[0]
        );
    }

    const ERROR: f64 = 0.027;
    const EPSILON: f64 = 1e-9;

    /// Every snapshot's own checks: counts, provenance, slivers, the dense
    /// two-sided oracle against the report and the bound, topology, the
    /// material edge, texture deviation, and the fixtures' counts.
    fn check_snapshot(f: &Fixture, output: &super::SimplifyOutput, error: f64, level: usize, last: bool) {
        let epsilon = EPSILON;
        // Eleven subdivisions along both barycentric axes, including all edges
        // and vertices (78 lattice points, independent of dyadic cell corners).
        let mut weights = Vec::new();
        for i in 0..=11 {
            for j in 0..=11 - i {
                weights.push([(11 - i - j) as f64 / 11.0, i as f64 / 11.0, j as f64 / 11.0]);
            }
        }
        // Past the bound the oracle may only overstate: see within_reach.
        let reach = error * (1.0 + 1e-6) + epsilon;
        assert_eq!(
            output.report.faces,
            output.triangles.len(),
            "{}: reported face count",
            f.name
        );
        assert_eq!(
            output.groups.len(),
            output.triangles.len(),
            "{}: output group count",
            f.name
        );
        assert!(
            output.report.max_forward <= error,
            "{}: level {level}: reported forward bound {} exceeds {error}",
            f.name, output.report.max_forward
        );
        assert!(
            output.report.max_backward <= error,
            "{}: level {level}: reported backward bound {} exceeds {error}",
            f.name, output.report.max_backward
        );
        for (triangle, &group) in output.triangles.iter().zip(&output.groups) {
            for &(v, w) in triangle {
                assert!(
                    (v as usize) < f.positions.len() && (w as usize) < f.uv.len(),
                    "{}: output source indices",
                    f.name
                );
                assert!(
                    f.triangles.iter().zip(&f.wedges).zip(&f.groups).any(
                        |((t, wedges), &g)| {
                            g == group && (0..3).any(|i| t[i] == v && wedges[i] == w)
                        }
                    ),
                    "{}: corner wedge belongs to its source vertex and group",
                    f.name
                );
            }
            // Mesh validation refuses a face whose height over its longest
            // edge is within 1e-12 of that edge.
            let p = triangle.map(|c| f.positions[c.0 as usize]);
            let (ab, ac) = (minus(p[1], p[0]), minus(p[2], p[0]));
            let n = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
            let longest = [ab, ac, minus(p[2], p[1])].map(|e| inner(e, e)).into_iter().fold(0.0, f64::max);
            assert!(
                inner(n, n).sqrt() > 1e-12 * longest,
                "{}: output face {triangle:?} is a sliver",
                f.name
            );
        }
        let source: Vec<_> = f
            .triangles
            .iter()
            .map(|t| OracleTriangle::new(t.map(|v| f.positions[v as usize])))
            .collect();
        let reduced: Vec<_> = output
            .triangles
            .iter()
            .map(|t| OracleTriangle::new(t.map(|c| f.positions[c.0 as usize])))
            .collect();
        let forward = dense_distance(&reduced, &source, &weights, reach);
        let backward = dense_distance(&source, &reduced, &weights, reach);
        assert!(
            forward <= output.report.max_forward + epsilon,
            "{}: level {level}: dense forward surface error {forward} exceeds reported {}",
            f.name, output.report.max_forward
        );
        assert!(
            forward <= error + epsilon,
            "{}: level {level}: dense forward surface error {forward} exceeds {error}",
            f.name
        );
        assert!(
            backward <= output.report.max_backward + epsilon,
            "{}: level {level}: dense backward surface error {backward} exceeds reported {}",
            f.name, output.report.max_backward
        );
        assert!(
            backward <= error + epsilon,
            "{}: level {level}: dense backward surface error {backward} exceeds {error}",
            f.name
        );
        if f.closed {
            let positions: Vec<_> = f.positions.iter().map(|p| p.map(|v| v as f32)).collect();
            let original: Vec<_> = f.triangles.iter().flatten().copied().collect();
            let indices: Vec<_> = output.triangles.iter().flatten().map(|c| c.0).collect();
            let before = audit_mesh_topology(&positions, &original);
            let after = audit_mesh_topology(&positions, &indices);
            assert_eq!(
                (before.boundary_edges, before.nonmanifold_edges),
                (0, 0),
                "{}: closed fixture construction",
                f.name
            );
            assert_eq!(
                after.boundary_edges, 0,
                "{}: closed output has boundary edges",
                f.name
            );
            assert_eq!(
                after.nonmanifold_edges, 0,
                "{}: closed output has non-manifold edges",
                f.name
            );
            assert_eq!(
                after.inconsistent_edges, 0,
                "{}: output winding consistency",
                f.name
            );
        }
        if last && f.name == "torus" {
            assert_eq!(f.triangles.len(), 1152, "torus: source resolution");
            assert!(
                output.triangles.len() <= 120,
                "torus: reduction count {} exceeds 120",
                output.triangles.len()
            );
            assert_eq!(
                output.report.stop,
                SimplifyStop::Target,
                "torus: target stop reason"
            );
        }
        if last && f.name == "sliver fan" {
            assert_eq!(output.triangles.len(), 12, "sliver fan: one collapse, without the sliver");
        }
        if last && f.name == "rounded box" {
            assert_eq!(
                output.triangles.len(),
                12,
                "rounded box: reduction count must be 12"
            );
            assert_eq!(
                output.report.stop,
                SimplifyStop::Target,
                "rounded box: target stop reason"
            );
        }
        if f.name == "two-material plate" {
            let mut edges: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
            for (t, &group) in output.triangles.iter().zip(&output.groups) {
                for i in 0..3 {
                    let (a, b) = (t[i].0, t[(i + 1) % 3].0);
                    edges.entry((a.min(b), a.max(b))).or_default().push(group);
                }
            }
            let mut boundary_vertices = 0;
            for ((a, b), groups) in edges {
                if groups.len() == 2 && groups[0] != groups[1] {
                    for v in [a, b] {
                        let p = f.positions[v as usize];
                        let d = p[0].hypot(p[2]);
                        assert!(
                            d <= error + epsilon,
                            "two-material plate: material boundary distance {d}"
                        );
                        boundary_vertices += 1;
                    }
                }
            }
            assert!(
                boundary_vertices > 0,
                "two-material plate: material boundary disappeared"
            );
        }
        if f.uv_bound.iter().any(|&v| v) {
            let source_uv: Vec<_> = f
                .wedges
                .iter()
                .map(|t| t.map(|w| f.uv[w as usize]))
                .collect();
            let reduced_uv: Vec<_> = output
                .triangles
                .iter()
                .map(|t| t.map(|c| f.uv[c.1 as usize]))
                .collect();
            let forward =
                texture_distance(&reduced, &reduced_uv, &source, &source_uv, &weights, reach);
            let backward =
                texture_distance(&source, &source_uv, &reduced, &reduced_uv, &weights, reach);
            // The backward report holds the UV-overlap certificate. Its
            // affine deviation also bounds the reverse texture mapping.
            assert!(
                forward <= output.report.max_backward + epsilon,
                "{}: level {level}: dense forward texture deviation {forward} exceeds reported {}",
                f.name, output.report.max_backward
            );
            assert!(
                forward <= error + epsilon,
                "{}: level {level}: dense forward texture deviation {forward} exceeds {error}",
                f.name
            );
            assert!(
                backward <= output.report.max_backward + epsilon,
                "{}: level {level}: dense backward texture deviation {backward} exceeds reported {}",
                f.name, output.report.max_backward
            );
            assert!(
                backward <= error + epsilon,
                "{}: level {level}: dense backward texture deviation {backward} exceeds {error}",
                f.name
            );
        }
    }

    /// One fixture through three rising bounds: every snapshot's checks, the
    /// running maxima, the re-queue of edges a smaller bound rejected, and,
    /// with `scaled`, invariance under a uniform scale.
    fn check_levels(f: &Fixture, scaled: bool) {
        let error = ERROR;
        // Equal targets deliberately exhaust the tighter-bound passes on
        // the torus and sphere, so the next pass must retry rejected edges.
        let levels = [
            SimplifyLevel { max_error: error / 4.0, target_faces: f.target },
            SimplifyLevel { max_error: error / 2.0, target_faces: f.target },
            SimplifyLevel { max_error: error, target_faces: f.target },
        ];
        let outputs = simplify_levels(&f.input(), &levels, &mut |_| true)
            .unwrap_or_else(|err| panic!("{}: valid input rejected: {err}", f.name))
            .unwrap_or_else(|| panic!("{}: unexpected cancellation", f.name));
        assert_eq!(outputs.len(), levels.len(), "{}: one snapshot per level", f.name);
        let mut previous = f.triangles.len();
        for (i, (output, level)) in outputs.iter().zip(&levels).enumerate() {
            assert!(output.report.faces <= previous, "{}: face counts cannot increase at level {}", f.name, i + 1);
            if i > 0 {
                assert!(output.report.max_forward >= outputs[i - 1].report.max_forward,
                    "{}: level {} keeps the running forward maximum", f.name, i + 1);
                assert!(output.report.max_backward >= outputs[i - 1].report.max_backward,
                    "{}: level {} keeps the running backward maximum", f.name, i + 1);
            }
            if previous <= level.target_faces {
                assert_eq!(output.report.stop, SimplifyStop::Target, "{}: already-met target at level {}", f.name, i + 1);
            }
            previous = output.report.faces;
            check_snapshot(f, output, level.max_error, i + 1, i + 1 == levels.len());
        }
        if f.name == "torus" || f.name == "UV sphere" {
            assert_eq!(outputs[0].report.stop, SimplifyStop::NoCandidate,
                "{}: level 1 must exhaust the smaller bound", f.name);
            assert!(outputs[0].report.faces > levels[0].target_faces,
                "{}: level 1 must stop above its target", f.name);
            assert!(outputs[1].report.faces < outputs[0].report.faces,
                "{}: an edge rejected under the smaller bound was re-queued and collapsed under the larger one", f.name);
        }
        // One uniform scale: normalization makes every power of two alike.
        if scaled {
            let k = 16.0;
            let positions: Vec<_> = f.positions.iter().map(|p| p.map(|v| v * k)).collect();
            let input = SimplifyInput { positions: &positions, ..f.input() };
            let scaled_levels: Vec<_> = levels.iter().map(|level| SimplifyLevel {
                max_error: level.max_error * k, target_faces: level.target_faces,
            }).collect();
            let scaled = simplify_levels(&input, &scaled_levels, &mut |_| true)
                .unwrap_or_else(|err| panic!("{}: scaled input rejected at scale {k}: {err}", f.name))
                .unwrap_or_else(|| panic!("{}: unexpected cancellation at scale {k}", f.name));
            assert_eq!(scaled.len(), outputs.len(), "{}: scaled snapshot count", f.name);
            for (i, (scaled, output)) in scaled.iter().zip(&outputs).enumerate() {
                assert_eq!(scaled.triangles, output.triangles,
                    "{}: level {} is invariant under uniform scale {k}: triangles", f.name, i + 1);
                assert_eq!(scaled.groups, output.groups,
                    "{}: level {} is invariant under uniform scale {k}: groups", f.name, i + 1);
                assert_eq!((scaled.report.faces, scaled.report.stop), (output.report.faces, output.report.stop),
                    "{}: level {} is invariant under uniform scale {k}: count and stop", f.name, i + 1);
                for (name, actual, expected) in [
                    ("forward", scaled.report.max_forward, output.report.max_forward * k),
                    ("backward", scaled.report.max_backward, output.report.max_backward * k),
                ] {
                    assert!((actual - expected).abs() <= 1e-9 * expected.abs(),
                        "{}: level {} is invariant under uniform scale {k}: {name} report {actual} differs from {expected}", f.name, i + 1);
                }
            }
        }
    }

    #[test]
    fn simplify_certified_source_bound() {
        let fixtures = [
            rounded_box(),
            sphere(),
            washer(),
            patch(false),
            patch(true),
            sliver_fan(),
            saddle(),
        ];
        let error = ERROR;
        // Scale invariance runs on the small fixtures, which between them
        // cover every path (UV charts, materials, slivers, curvature); the
        // sphere and the torus repeat it at many times the cost.
        for f in &fixtures {
            check_levels(f, f.name != "UV sphere");
        }

        // Shrinking authored bounds tighten the earlier snapshot too.
        let f = &fixtures[1];
        let shrinking = [
            SimplifyLevel { max_error: error, target_faces: f.target },
            SimplifyLevel { max_error: error / 2.0, target_faces: f.target },
        ];
        let outputs = simplify_levels(&f.input(), &shrinking, &mut |_| true).unwrap().unwrap();
        assert_eq!(outputs.len(), shrinking.len(), "{}: shrinking-bound snapshot count", f.name);
        for (i, output) in outputs.iter().enumerate() {
            check_snapshot(f, output, error / 2.0, i + 1, false);
        }
        assert!(outputs[1].report.faces <= outputs[0].report.faces, "{}: shrinking bounds cannot increase the face count", f.name);

        let f = &fixtures[3];
        assert!(
            simplify_levels(&f.input(), &[SimplifyLevel { max_error: error, target_faces: 0 }], &mut |_| false)
                .unwrap()
                .is_none(),
            "two-material plate: immediate cancellation"
        );
        let mut calls = 0;
        assert!(
            simplify_levels(&f.input(), &[SimplifyLevel { max_error: error, target_faces: 0 }], &mut |_| {
                calls += 1;
                calls < 8
            })
            .unwrap()
            .is_none(),
            "two-material plate: cancellation during setup"
        );
        assert!(
            simplify_levels(&f.input(), &[SimplifyLevel { max_error: error, target_faces: 0 }], &mut |faces| faces
                == f.triangles.len())
            .unwrap()
            .is_none(),
            "two-material plate: cancellation after a committed collapse"
        );
        let unchanged = simplify_levels(&f.input(), &[SimplifyLevel { max_error: 0.0, target_faces: f.triangles.len() }], &mut |_| true)
            .unwrap()
            .unwrap().remove(0);
        assert_eq!(
            unchanged.report.stop,
            SimplifyStop::Target,
            "two-material plate: already at target"
        );
        assert_eq!(
            unchanged.triangles.len(),
            f.triangles.len(),
            "two-material plate: already-at-target count"
        );

        let nonmanifold = Fixture::new(
            "non-manifold fan",
            vec![
                [0.0, 0.0, 0.0],
                [0.1, 0.0, 0.0],
                [0.0, 0.1, 0.0],
                [0.0, 0.0, 0.1],
                [0.0, -0.1, 0.0],
            ],
            vec![[0, 1, 2], [1, 0, 3], [0, 1, 4]],
            false,
            0,
        );
        assert!(
            simplify_levels(&nonmanifold.input(), &[SimplifyLevel { max_error: error, target_faces: 0 }], &mut |_| true)
                .unwrap()
                .is_some(),
            "non-manifold fan: valid input must not error or cancel"
        );
        let triangle = Fixture::new(
            "open triangle",
            vec![[0.0, 0.0, 0.0], [0.1, 0.0, 0.0], [0.0, 0.1, 0.0]],
            vec![[0, 1, 2]],
            false,
            0,
        );
        let exhausted = simplify_levels(&triangle.input(), &[SimplifyLevel { max_error: error, target_faces: 0 }], &mut |_| true)
            .unwrap()
            .unwrap().remove(0);
        assert_eq!(
            exhausted.report.stop,
            SimplifyStop::NoCandidate,
            "open triangle: exhausted queue stop reason"
        );
        assert_eq!(
            exhausted.triangles.len(),
            1,
            "open triangle: last surface must survive"
        );
        for invalid_error in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(
                simplify_levels(&f.input(), &[
                    SimplifyLevel { max_error: error, target_faces: usize::MAX },
                    SimplifyLevel { max_error: invalid_error, target_faces: usize::MAX },
                ], &mut |_| panic!("all level bounds must be validated before cancellation")).is_err(),
                "two-material plate: invalid error bound must fail validation"
            );
        }
        let mut invalid = f.input();
        invalid.wedges = &[];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| panic!(
                "two-material plate: validation must precede cancellation"
            ))
            .is_err(),
            "two-material plate: mismatched wedges must be an error"
        );
        let mut invalid = f.input();
        invalid.groups = &[];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| true).is_err(),
            "two-material plate: mismatched groups must be an error"
        );
        let mut invalid = f.input();
        invalid.positions = &[[f64::NAN, 0.0, 0.0]];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| true).is_err(),
            "two-material plate: non-finite position must be an error"
        );
        let mut invalid = f.input();
        invalid.wedge_uv = &[[f64::INFINITY, 0.0]];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| true).is_err(),
            "two-material plate: non-finite UV must be an error"
        );
        let mut invalid = f.input();
        invalid.positions = &f.positions[..1];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| true).is_err(),
            "two-material plate: out-of-range position must be an error"
        );
        let mut invalid = f.input();
        invalid.wedge_uv = &f.uv[..1];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| true).is_err(),
            "two-material plate: out-of-range wedge must be an error"
        );
        let mut invalid = f.input();
        invalid.uv_bound = &[false];
        assert!(
            simplify_levels(&invalid, &[SimplifyLevel { max_error: error, target_faces: usize::MAX }], &mut |_| true).is_err(),
            "two-material plate: uncovered group must be an error"
        );
    }

    /// The torus alone: its 48 x 12 grid is what the count <= 120 and its
    /// Target stop are about, and it exhausts the smallest bound at level 1.
    #[test]
    fn simplify_certified_torus_meets_its_count() {
        check_levels(&torus(), false);
    }
}
