//! Half-edge LOD simplification with sampled, two-sided source-surface bounds.
//! Positions and wedges in the result always refer to the immutable input.

use crate::spatial::{tri_distance_sq, BinGrid, BIN_DILATE, BIN_SIZE, GRID_RES};
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

#[derive(Clone, Copy, Debug)]
pub struct SimplifyReport {
    pub faces: usize,
    pub max_forward: f64,
    /// Includes texture deviation and source-curve distance where applicable.
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

fn uv_weights(p: [f64; 2], t: [[f64; 2]; 3]) -> Option<[f64; 3]> {
    let orient = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    };
    let det = orient(t[0], t[1], t[2]);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let w = [
        orient(t[1], t[2], p) / det,
        orient(t[2], t[0], p) / det,
        orient(t[0], t[1], p) / det,
    ];
    w.iter().all(|v| v.is_finite() && *v >= 0.0).then_some(w)
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
    samples: Vec<usize>,
}

struct Sample {
    vertex: u32,
    wedge: u32,
    group: u32,
    face: usize,
}

/// One grid for the immutable source, with a group filter at query time.
struct Source {
    grid: BinGrid,
    triangles: Vec<[[f32; 3]; 3]>,
    seen: Vec<bool>,
    visited: Vec<usize>,
}

impl Source {
    fn bin(x: f64) -> i64 {
        (((x + 1.0) / BIN_SIZE).floor() as i64).clamp(0, GRID_RES - 1)
    }

    fn distance(
        &mut self,
        point: Point,
        group: u32,
        groups: &[u32],
        bound: f64,
        faces: usize,
        ctl: &mut dyn FnMut(usize) -> bool,
    ) -> Result<f64, ()> {
        for id in self.visited.drain(..) {
            self.seen[id] = false;
        }
        let p = point.map(|v| v as f32);
        let pb = p.map(|v| Self::bin(v as f64));
        let mut best = f64::INFINITY;
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
                            let d = tri_distance_sq(p, &self.triangles[id]) as f64;
                            if d.is_finite() {
                                best = best.min(d);
                            }
                        }
                    }
                }
            }
            let mut boundary = f64::INFINITY;
            for a in 0..3 {
                if lo[a] > 0 {
                    boundary = boundary.min(p[a] as f64 - (-1.0 + lo[a] as f64 * BIN_SIZE));
                }
                if hi[a] < GRID_RES - 1 {
                    boundary = boundary.min((-1.0 + (hi[a] + 1) as f64 * BIN_SIZE) - p[a] as f64);
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
        Ok(best.sqrt())
    }
}

struct State<'a> {
    input: &'a SimplifyInput<'a>,
    positions: Vec<Point>,
    faces: Vec<Face>,
    rings: Vec<BTreeSet<usize>>,
    edges: HashMap<Edge, BTreeSet<usize>>,
    spans: HashMap<Edge, Vec<u32>>,
    samples: Vec<Sample>,
    vertex_samples: Vec<Vec<usize>>,
    quadrics: Vec<Quadric>,
    versions: Vec<u64>,
    alive: Vec<bool>,
    count: usize,
}

fn validate(input: &SimplifyInput, max_error: f64) -> Result<(Vec<Point>, f64), String> {
    if !max_error.is_finite() || max_error < 0.0 {
        return Err("max_error must be finite and nonnegative".into());
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
            samples: Vec::new(),
            vertex_samples: vec![Vec::new(); input.positions.len()],
            quadrics: vec![Quadric::default(); input.positions.len()],
            versions: vec![0; input.positions.len()],
            alive: vec![true; input.positions.len()],
            count,
        };
        let mut sample_ids = HashMap::new();
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
                samples: Vec::new(),
            });
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
            for (vertex, wedge) in corners {
                // A seam vertex needs a sample for each UV and each group.
                let key = (
                    vertex,
                    group,
                    if input.uv_bound[group as usize] {
                        wedge
                    } else {
                        0
                    },
                );
                sample_ids.entry(key).or_insert_with(|| {
                    let sample_id = state.samples.len();
                    state.samples.push(Sample {
                        vertex,
                        wedge,
                        group,
                        face: id,
                    });
                    state.faces[id].samples.push(sample_id);
                    state.vertex_samples[vertex as usize].push(sample_id);
                    sample_id
                });
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
    assignments: Vec<(usize, usize)>,
    curve: Option<(Edge, Vec<u32>)>,
    forward: f64,
    backward: f64,
}

impl State<'_> {
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
                    || area_sq <= 0.0
                    || !orientation.is_finite()
                    || orientation <= 0.0
                {
                    return Ok(None);
                }
                for weights in [
                    [0.5, 0.5, 0.0],
                    [0.0, 0.5, 0.5],
                    [0.5, 0.0, 0.5],
                    [1.0 / 3.0; 3],
                ] {
                    let d = source.distance(
                        mix(t, weights),
                        old.group,
                        self.input.groups,
                        bound,
                        self.count,
                        ctl,
                    )?;
                    if !d.is_finite() || d > bound {
                        return Ok(None);
                    }
                    result.forward = result.forward.max(d);
                }
            }
            result.ring.push((id, corners));
        }

        let mut samples = BTreeSet::new();
        for &id in &self.rings[from as usize] {
            for &sample in &self.faces[id].samples {
                if !ctl(self.count) {
                    return Err(());
                }
                samples.insert(sample);
            }
        }
        samples.extend(self.vertex_samples[from as usize].iter().copied());
        for sample_id in samples {
            let sample = &self.samples[sample_id];
            let p = self.positions[sample.vertex as usize];
            let textured = self.input.uv_bound[sample.group as usize];
            let mut nearest = None;
            let mut best = f64::INFINITY;
            for &(id, corners) in &result.ring {
                if !ctl(self.count) {
                    return Err(());
                }
                if self.faces[id].group != sample.group {
                    continue;
                }
                let t = self.geometry(corners);
                let d = if textured {
                    let uv = corners.map(|c| self.input.wedge_uv[c.1 as usize]);
                    if let Some(weights) =
                        uv_weights(self.input.wedge_uv[sample.wedge as usize], uv)
                    {
                        let delta = sub(p, mix(t, weights));
                        dot(delta, delta)
                    } else {
                        continue;
                    }
                } else {
                    distance_sq(p, t)
                };
                if d < best {
                    best = d;
                    nearest = Some(id);
                }
            }
            let d = best.sqrt();
            if !d.is_finite() || d > bound {
                return Ok(None);
            }
            let Some(nearest) = nearest else {
                return Ok(None);
            };
            result.backward = result.backward.max(d);
            // Keep the owner that supplied the bound. For textured groups it
            // must contain this UV, so a later change to that mapping rechecks
            // the sample even when a different face is geometrically closer.
            result.assignments.push((sample_id, nearest));
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

        let moving: BTreeSet<_> = collapse
            .assignments
            .iter()
            .map(|&(sample, _)| sample)
            .collect();
        let owners: BTreeSet<_> = moving.iter().map(|&id| self.samples[id].face).collect();
        for id in owners {
            affected.extend(self.faces[id].corners.map(|c| c.0));
            self.faces[id]
                .samples
                .retain(|sample| !moving.contains(sample));
        }
        for (sample, face) in collapse.assignments {
            self.samples[sample].face = face;
            self.faces[face].samples.push(sample);
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
        // Endpoints of changed faces, sample owners, and the new target ring
        // cover every local dependency of check(). Rejections are requeued only
        // here, after such a dependency changes, never immediately on rejection.
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

/// Simplify one welded part. The error is the specified sampled surface bound,
/// not a bound on vertex displacement or on quadric cost. `ctl` may cancel;
/// invalid inputs are always errors, even when no reduction was requested.
pub fn simplify(
    input: &SimplifyInput,
    max_error: f64,
    target_faces: usize,
    ctl: &mut dyn FnMut(usize) -> bool,
) -> Result<Option<SimplifyOutput>, String> {
    let (positions, scale) = validate(input, max_error)?;
    let bound = max_error * scale;
    if !ctl(input.triangles.len()) {
        return Ok(None);
    }
    let mut report = SimplifyReport {
        faces: input.triangles.len(),
        max_forward: 0.0,
        max_backward: 0.0,
        stop: SimplifyStop::Target,
    };
    if input.triangles.len() <= target_faces {
        return Ok(Some(SimplifyOutput {
            triangles: input
                .triangles
                .iter()
                .zip(input.wedges)
                .map(|(t, w)| std::array::from_fn(|i| (t[i], w[i])))
                .collect(),
            groups: input.groups.to_vec(),
            report,
        }));
    }
    let Some(mut state) = State::new(input, positions, ctl) else {
        return Ok(None);
    };
    let mut triangles = Vec::with_capacity(input.triangles.len());
    for tri in input.triangles {
        if !ctl(state.count) {
            return Ok(None);
        }
        triangles.push(tri.map(|v| state.positions[v as usize].map(|p| p as f32)));
    }
    let mut source = Source {
        grid: BinGrid::build(&triangles),
        seen: vec![false; triangles.len()],
        visited: Vec::new(),
        triangles,
    };
    let mut heap = BinaryHeap::new();
    let mut edges: Vec<_> = state.edges.keys().copied().collect();
    edges.sort_unstable();
    for (a, b) in edges {
        if !ctl(state.count) {
            return Ok(None);
        }
        state.enqueue(a, b, &mut heap);
        state.enqueue(b, a, &mut heap);
    }
    while state.count > target_faces {
        if !ctl(state.count) {
            return Ok(None);
        }
        let Some(candidate) = heap.pop() else {
            report.stop = SimplifyStop::NoCandidate;
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
                report.max_forward = report.max_forward.max(collapse.forward);
                report.max_backward = report.max_backward.max(collapse.backward);
                state.commit(from, to, collapse, &mut heap);
            }
        }
    }
    if !ctl(state.count) {
        return Ok(None);
    }
    report.faces = state.count;
    report.max_forward /= scale;
    report.max_backward /= scale;
    let mut output = SimplifyOutput {
        triangles: Vec::with_capacity(state.count),
        groups: Vec::with_capacity(state.count),
        report,
    };
    for face in state.faces {
        if face.alive {
            output.triangles.push(face.corners);
            output.groups.push(face.group);
        }
    }
    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    use super::{simplify, SimplifyInput, SimplifyStop};
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

    fn dense_distance(from: &[OracleTriangle], to: &[OracleTriangle], weights: &[[f64; 3]]) -> f64 {
        let mut maximum: f64 = 0.0;
        for triangle in from {
            for &w in weights {
                let p = triangle.point(w);
                let mut nearest = f64::INFINITY;
                // Every other triangle is tested, without a spatial index,
                // distance threshold, or early exit at a small distance.
                for other in to {
                    nearest = nearest.min(other.distance_sq(p));
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
    ) -> f64 {
        let mut maximum: f64 = 0.0;
        for (tri, uv) in from.iter().zip(from_uv) {
            for &w in weights {
                let p = tri.point(w);
                let sample_uv =
                    std::array::from_fn(|a| uv[0][a] * w[0] + uv[1][a] * w[1] + uv[2][a] * w[2]);
                let mut nearest = f64::INFINITY;
                for (other, other_uv) in to.iter().zip(to_uv) {
                    if let Some(bary) = oracle_uv_weights(sample_uv, *other_uv) {
                        let delta = minus(p, other.point(bary));
                        nearest = nearest.min(inner(delta, delta));
                    }
                }
                maximum = maximum.max(nearest);
            }
        }
        maximum.sqrt()
    }

    #[test]
    fn simplify_sampled_source_bound() {
        let fixtures = [
            torus(),
            rounded_box(),
            sphere(),
            washer(),
            patch(false),
            patch(true),
        ];
        let error = 0.027;
        let epsilon = 1e-6;
        // Ten subdivisions along both barycentric axes, including all edges
        // and vertices (66 lattice points in each triangular 10 x 10 grid).
        let mut weights = Vec::new();
        for i in 0..=10 {
            for j in 0..=10 - i {
                weights.push([(10 - i - j) as f64 / 10.0, i as f64 / 10.0, j as f64 / 10.0]);
            }
        }
        for f in &fixtures {
            let output = simplify(&f.input(), error, f.target, &mut |_| true)
                .unwrap_or_else(|err| panic!("{}: valid input rejected: {err}", f.name))
                .unwrap_or_else(|| panic!("{}: unexpected cancellation", f.name));
            if f.name == "two-material plate" || f.name == "torus" {
                for k in [0.0625, 16.0] {
                    let positions: Vec<_> = f.positions.iter().map(|p| p.map(|v| v * k)).collect();
                    let input = SimplifyInput {
                        positions: &positions,
                        ..f.input()
                    };
                    let scaled = simplify(&input, error * k, f.target, &mut |_| true)
                        .unwrap_or_else(|err| {
                            panic!("{}: scaled input rejected at scale {k}: {err}", f.name)
                        })
                        .unwrap_or_else(|| {
                            panic!("{}: unexpected cancellation at scale {k}", f.name)
                        });
                    assert_eq!(
                        scaled.triangles, output.triangles,
                        "{}: simplification is invariant under uniform scale {k}: triangles",
                        f.name
                    );
                    assert_eq!(
                        scaled.groups, output.groups,
                        "{}: simplification is invariant under uniform scale {k}: groups",
                        f.name
                    );
                    assert_eq!(
                        (scaled.report.faces, scaled.report.stop),
                        (output.report.faces, output.report.stop),
                        "{}: simplification is invariant under uniform scale {k}: count and stop",
                        f.name
                    );
                    for (name, actual, expected) in [
                        ("forward", scaled.report.max_forward, output.report.max_forward * k),
                        ("backward", scaled.report.max_backward, output.report.max_backward * k),
                    ] {
                        assert!(
                            (actual - expected).abs() <= 1e-9 * expected.abs(),
                            "{}: simplification is invariant under uniform scale {k}: {name} report {actual} differs from {expected}",
                            f.name
                        );
                    }
                }
            }
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
                "{}: reported forward bound",
                f.name
            );
            assert!(
                output.report.max_backward <= error,
                "{}: reported backward bound",
                f.name
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
            let forward = dense_distance(&reduced, &source, &weights);
            let backward = dense_distance(&source, &reduced, &weights);
            assert!(
                forward <= error + epsilon,
                "{}: dense forward surface error {forward} exceeds {error}",
                f.name
            );
            assert!(
                backward <= error + epsilon,
                "{}: dense backward surface error {backward} exceeds {error}",
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
            if f.name == "torus" {
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
            if f.name == "rounded box" {
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
                    texture_distance(&reduced, &reduced_uv, &source, &source_uv, &weights);
                let backward =
                    texture_distance(&source, &source_uv, &reduced, &reduced_uv, &weights);
                assert!(
                    forward <= error + epsilon,
                    "textured chart: dense forward texture deviation {forward}"
                );
                assert!(
                    backward <= error + epsilon,
                    "textured chart: dense backward texture deviation {backward}"
                );
            }
        }

        let f = &fixtures[4];
        assert!(
            simplify(&f.input(), error, 0, &mut |_| false)
                .unwrap()
                .is_none(),
            "two-material plate: immediate cancellation"
        );
        let mut calls = 0;
        assert!(
            simplify(&f.input(), error, 0, &mut |_| {
                calls += 1;
                calls < 8
            })
            .unwrap()
            .is_none(),
            "two-material plate: cancellation during setup"
        );
        assert!(
            simplify(&f.input(), error, 0, &mut |faces| faces
                == f.triangles.len())
            .unwrap()
            .is_none(),
            "two-material plate: cancellation after a committed collapse"
        );
        let unchanged = simplify(&f.input(), 0.0, f.triangles.len(), &mut |_| true)
            .unwrap()
            .unwrap();
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
            simplify(&nonmanifold.input(), error, 0, &mut |_| true)
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
        let exhausted = simplify(&triangle.input(), error, 0, &mut |_| true)
            .unwrap()
            .unwrap();
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
                simplify(&f.input(), invalid_error, usize::MAX, &mut |_| true).is_err(),
                "two-material plate: invalid error bound must fail validation"
            );
        }
        let mut invalid = f.input();
        invalid.wedges = &[];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| panic!(
                "two-material plate: validation must precede cancellation"
            ))
            .is_err(),
            "two-material plate: mismatched wedges must be an error"
        );
        let mut invalid = f.input();
        invalid.groups = &[];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| true).is_err(),
            "two-material plate: mismatched groups must be an error"
        );
        let mut invalid = f.input();
        invalid.positions = &[[f64::NAN, 0.0, 0.0]];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| true).is_err(),
            "two-material plate: non-finite position must be an error"
        );
        let mut invalid = f.input();
        invalid.wedge_uv = &[[f64::INFINITY, 0.0]];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| true).is_err(),
            "two-material plate: non-finite UV must be an error"
        );
        let mut invalid = f.input();
        invalid.positions = &f.positions[..1];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| true).is_err(),
            "two-material plate: out-of-range position must be an error"
        );
        let mut invalid = f.input();
        invalid.wedge_uv = &f.uv[..1];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| true).is_err(),
            "two-material plate: out-of-range wedge must be an error"
        );
        let mut invalid = f.input();
        invalid.uv_bound = &[false];
        assert!(
            simplify(&invalid, error, usize::MAX, &mut |_| true).is_err(),
            "two-material plate: uncovered group must be an error"
        );
    }
}
