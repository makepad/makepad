//! Turns a surface into charts. Regions start as large as creases and seams
//! allow; a region is flattened, measured and split in two until every piece
//! is a topological disk within the distortion limit. Top-down keeps a
//! developable run (a cylinder wall, a long rounded trim) in as few pieces as
//! it needs, where growing outward under a fixed normal cone would chop it.

use crate::solve::{self, Csr, C};
use crate::topology::{cross, dot, length, normalized, sub, Topology, NONE};
use crate::{Chart, ChartOptions, Charting, Checkpoint, MeshInput, UnwrapError};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// A region all of whose normals sit within this cosine of their mean is
/// projected onto its plane rather than solved.
const FLAT_COS: f64 = 0.999;

/// Share of a chart's surface the distortion limit may ignore, so that a few
/// tiny triangles at a pin or a bad sliver do not split a clean chart.
const STRETCH_QUANTILE: f64 = 0.95;

/// `atan2` of a non-negative `y`, from arithmetic alone so that results do not
/// depend on a platform's math library. Good to about 0.005 radians, which is
/// all the curvature gate needs.
fn angle_between(y: f64, x: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI};
    if y <= 0.0 && x == 0.0 {
        return 0.0;
    }
    let atan01 = |t: f64| t * (FRAC_PI_2 * 0.5 + 0.273 * (1.0 - t));
    let ax = x.abs();
    let base = if y <= ax {
        atan01(y / ax)
    } else {
        FRAC_PI_2 - atan01(ax / y)
    };
    if x < 0.0 {
        PI - base
    } else {
        base
    }
}

const UNSET: u8 = u8::MAX;

struct Region {
    atoms: Vec<u32>,
    depth: u32,
}

struct Flattened {
    /// Chart-space position of each corner of `tris`.
    corners: Vec<[f64; 2]>,
    stretch: f64,
    flat: bool,
}

struct Segmenter<'a, 'b> {
    topology: &'b Topology<'a>,
    options: &'b ChartOptions,
    /// Region stamp per atom; an edge is internal when both sides match.
    region_of: Vec<u32>,
    next_region: u32,
    parent: Vec<u32>,
    vertex_of: Vec<u32>,
    /// Piece per atom during a split; `UNSET` outside it.
    side: Vec<u8>,
    limit_gate: f64,
}

/// Runs the whole segmentation and flattening.
pub(crate) fn charts(
    input: &MeshInput<'_>,
    options: &ChartOptions,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Charting, UnwrapError> {
    let topology = Topology::build(input, options)?;
    let triangles = topology.triangle_count();
    if !checkpoint(triangles as u64) {
        return Err(UnwrapError::Cancelled);
    }
    let s = options.max_stretch.max(1.0 + 1.0e-6);
    let mut segmenter = Segmenter {
        topology: &topology,
        options,
        region_of: vec![NONE; topology.atom_count()],
        next_region: 0,
        parent: vec![0; triangles * 3],
        vertex_of: vec![NONE; triangles * 3],
        side: vec![UNSET; topology.atom_count()],
        // A geodesic cap with this much total curvature cannot be conformally
        // flattened with its rim-to-centre scale ratio under s squared.
        limit_gate: 4.0 * std::f64::consts::PI * (1.0 - 1.0 / (s * s)),
    };
    let mut corner_uv = vec![[0.0; 2]; triangles * 3];
    let mut triangle_chart = vec![NONE; triangles];
    let mut accepted: Vec<(Vec<u32>, bool, f64)> = Vec::new();
    let mut stack = segmenter.base_regions();
    let depth_budget = 2 * (usize::BITS - topology.atom_count().leading_zeros()) + 8;
    while let Some(region) = stack.pop() {
        if !checkpoint(region.atoms.len() as u64) {
            return Err(UnwrapError::Cancelled);
        }
        if let Some((tris, flattened)) = segmenter.flatten(&region, checkpoint)? {
            for (i, &t) in tris.iter().enumerate() {
                for k in 0..3 {
                    corner_uv[t as usize * 3 + k] = flattened.corners[i * 3 + k];
                }
            }
            accepted.push((tris, flattened.flat, flattened.stretch));
        } else {
            let balanced = region.depth >= depth_budget;
            for part in segmenter.split(&region, balanced) {
                stack.push(part);
            }
        }
    }
    // Stable order whatever the split order was.
    accepted.sort_unstable_by_key(|c| c.0.iter().copied().min());
    let mut charts = Vec::with_capacity(accepted.len());
    for (tris, flat, stretch) in accepted {
        let index = charts.len() as u32;
        let mut area = 0.0;
        for &t in &tris {
            triangle_chart[t as usize] = index;
            area += topology.area[t as usize];
        }
        charts.push(Chart { triangles: tris, area, flat, stretch });
    }
    attach_collapsed(&topology, &mut corner_uv, &mut triangle_chart, &mut charts);
    Ok(Charting { corner_uv, triangle_chart, charts })
}

/// Triangles with welded-together corners have no surface. Each takes the uv
/// its vertices already have in a neighbouring chart; one with no neighbour at
/// all joins a shared point chart, so that every triangle still has a uv.
fn attach_collapsed(
    topology: &Topology<'_>,
    corner_uv: &mut [[f64; 2]],
    triangle_chart: &mut [u32],
    charts: &mut Vec<Chart>,
) {
    let triangles = topology.triangle_count();
    if !topology.collapsed.iter().any(|c| *c) {
        return;
    }
    let mut known: Vec<(u32, u32, u32)> = Vec::new();
    for t in 0..triangles {
        if !topology.collapsed[t] {
            for k in 0..3 {
                let corner = t * 3 + k;
                known.push((topology.welded[corner], triangle_chart[t], corner as u32));
            }
        }
    }
    known.sort_unstable();
    let mut orphans = Vec::new();
    for t in 0..triangles {
        if !topology.collapsed[t] {
            continue;
        }
        let vertices = [0, 1, 2].map(|k| topology.welded[t * 3 + k]);
        let found = vertices.iter().find_map(|&v| {
            let at = known.partition_point(|e| e.0 < v);
            known.get(at).filter(|e| e.0 == v).map(|e| (e.1, e.2))
        });
        let Some((chart, any_corner)) = found else {
            orphans.push(t as u32);
            continue;
        };
        triangle_chart[t] = chart;
        for k in 0..3 {
            let v = vertices[k];
            let at = known.partition_point(|e| (e.0, e.1) < (v, chart));
            let source = known
                .get(at)
                .filter(|e| e.0 == v && e.1 == chart)
                .map_or(any_corner, |e| e.2);
            corner_uv[t * 3 + k] = corner_uv[source as usize];
        }
        charts[chart as usize].triangles.push(t as u32);
    }
    if !orphans.is_empty() {
        let index = charts.len() as u32;
        for &t in &orphans {
            triangle_chart[t as usize] = index;
        }
        charts.push(Chart { triangles: orphans, area: 0.0, flat: true, stretch: 1.0 });
    }
}

impl Segmenter<'_, '_> {
    /// Connected components across edges that are neither unpaired nor cut.
    fn base_regions(&mut self) -> Vec<Region> {
        let atoms: Vec<u32> = (0..self.topology.atom_count() as u32).collect();
        self.components(atoms, 0)
    }

    /// Splits `atoms` into connected pieces of the dual graph.
    fn components(&mut self, atoms: Vec<u32>, depth: u32) -> Vec<Region> {
        let mark = self.next_region;
        self.next_region += 1;
        for &a in &atoms {
            self.region_of[a as usize] = mark;
        }
        let mut result = Vec::new();
        let mut queue = Vec::new();
        for &start in &atoms {
            if self.region_of[start as usize] != mark {
                continue;
            }
            let piece = self.next_region;
            self.next_region += 1;
            self.region_of[start as usize] = piece;
            queue.clear();
            queue.push(start);
            let mut part = Vec::new();
            while let Some(a) = queue.pop() {
                part.push(a);
                for &t in self.topology.tris_of(a) {
                    for k in 0..3 {
                        let e = t as usize * 3 + k;
                        let o = self.topology.opposite[e];
                        if o == NONE || self.topology.cut[e] {
                            continue;
                        }
                        let other = self.topology.atom_of[(o / 3) as usize];
                        if self.region_of[other as usize] == mark {
                            self.region_of[other as usize] = piece;
                            queue.push(other);
                        }
                    }
                }
            }
            part.sort_unstable();
            result.push(Region { atoms: part, depth });
        }
        result
    }

    /// Flattens `region`, or returns `None` when it has to be split first.
    fn flatten(
        &mut self,
        region: &Region,
        checkpoint: &mut Checkpoint<'_>,
    ) -> Result<Option<(Vec<u32>, Flattened)>, UnwrapError> {
        let topology = self.topology;
        let mark = self.next_region;
        self.next_region += 1;
        let mut tris = Vec::new();
        for &a in &region.atoms {
            self.region_of[a as usize] = mark;
            tris.extend_from_slice(topology.tris_of(a));
        }
        tris.sort_unstable();
        let single = region.atoms.len() == 1;

        // Chart vertices: corners stay one vertex only when a fan of internal
        // edges connects them. Two fans touching at a welded vertex stay two.
        for &t in &tris {
            for k in 0..3 {
                let c = t * 3 + k;
                self.parent[c as usize] = c;
                self.vertex_of[c as usize] = NONE;
            }
        }
        let internal = |this: &Self, e: usize| -> Option<usize> {
            let o = topology.opposite[e];
            if o == NONE || topology.cut[e] {
                return None;
            }
            (this.region_of[topology.atom_of[(o / 3) as usize] as usize] == mark)
                .then_some(o as usize)
        };
        let next = |c: usize| c - c % 3 + (c + 1) % 3;
        let mut boundary_edges = 0usize;
        for &t in &tris {
            for k in 0..3 {
                let e = t as usize * 3 + k;
                match internal(self, e) {
                    Some(o) => {
                        self.union(e, next(o));
                        self.union(next(e), o);
                    }
                    None => boundary_edges += 1,
                }
            }
        }
        let mut positions = Vec::new();
        let mut corners = Vec::with_capacity(tris.len() * 3);
        for &t in &tris {
            for k in 0..3 {
                let c = t as usize * 3 + k;
                let root = self.find(c);
                if self.vertex_of[root] == NONE {
                    self.vertex_of[root] = positions.len() as u32;
                    positions.push(topology.corner_position(c));
                }
                corners.push(self.vertex_of[root]);
            }
        }

        // Boundary loops, and with them the Euler characteristic.
        let n = positions.len();
        let mut boundary_next = vec![NONE; n];
        for (i, &t) in tris.iter().enumerate() {
            for k in 0..3 {
                let e = t as usize * 3 + k;
                if internal(self, e).is_none() {
                    boundary_next[corners[i * 3 + k] as usize] = corners[i * 3 + (k + 1) % 3];
                }
            }
        }
        let mut loops = 0usize;
        let mut visited = vec![false; n];
        for v in 0..n {
            if boundary_next[v] == NONE || visited[v] {
                continue;
            }
            loops += 1;
            let mut at = v;
            while !visited[at] && boundary_next[at] != NONE {
                visited[at] = true;
                at = boundary_next[at] as usize;
            }
        }
        let faces = tris.len();
        let edges = (3 * faces + boundary_edges) / 2;
        let euler = n as i64 - edges as i64 + faces as i64;
        let genus_free = 2 - loops as i64 == euler;
        if !single && (loops == 0 || !genus_free) {
            return Ok(None);
        }

        // The plane of the chart.
        let mut mean = [0.0; 3];
        for &t in &tris {
            for d in 0..3 {
                mean[d] += topology.normal[t as usize][d] * topology.area[t as usize];
            }
        }
        let axis = normalized(mean);
        let flat = tris.iter().all(|&t| {
            topology.sliver[t as usize] || dot(topology.normal[t as usize], axis) >= FLAT_COS
        });
        let basis = plane_basis(axis);
        let projected: Vec<C> =
            positions.iter().map(|&p| [dot(p, basis.0), dot(p, basis.1)]).collect();

        if flat {
            let uv = projected;
            if !single && boundary_crosses(&uv, &tris, &corners, |i, k| {
                internal(self, tris[i] as usize * 3 + k).is_none()
            }) {
                return Ok(None);
            }
            return Ok(Some((
                tris,
                Flattened {
                    corners: corners.iter().map(|&v| uv[v as usize]).collect(),
                    stretch: 1.0,
                    flat: true,
                },
            )));
        }

        if !single && self.curvature(&positions, &corners, &boundary_next) > self.limit_gate {
            return Ok(None);
        }

        let Some(mut uv) =
            conformal(topology, &tris, &corners, &positions, projected, &boundary_next, checkpoint)
        else {
            return Err(UnwrapError::Cancelled);
        };
        let (stretch, flipped) = normalize_and_measure(topology, &tris, &corners, &positions, &mut uv);
        if !single {
            if flipped || stretch > self.options.max_stretch {
                return Ok(None);
            }
            if boundary_crosses(&uv, &tris, &corners, |i, k| {
                internal(self, tris[i] as usize * 3 + k).is_none()
            }) {
                return Ok(None);
            }
        }
        Ok(Some((
            tris,
            Flattened {
                corners: corners.iter().map(|&v| uv[v as usize]).collect(),
                stretch,
                flat: false,
            },
        )))
    }

    fn find(&mut self, mut c: usize) -> usize {
        while self.parent[c] as usize != c {
            let grand = self.parent[self.parent[c] as usize];
            self.parent[c] = grand;
            c = grand as usize;
        }
        c
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a.max(b)] = a.min(b) as u32;
        }
    }

    /// Total absolute angle defect at the chart's interior vertices.
    fn curvature(&self, positions: &[[f64; 3]], corners: &[u32], boundary_next: &[u32]) -> f64 {
        let mut angle = vec![0.0; positions.len()];
        for tri in corners.chunks_exact(3) {
            for k in 0..3 {
                let v = tri[k] as usize;
                let p = positions[v];
                let a = sub(positions[tri[(k + 1) % 3] as usize], p);
                let b = sub(positions[tri[(k + 2) % 3] as usize], p);
                angle[v] += angle_between(length(cross(a, b)), dot(a, b));
            }
        }
        let tau = std::f64::consts::TAU;
        angle
            .iter()
            .zip(boundary_next)
            .filter(|(_, b)| **b == NONE)
            .map(|(a, _)| (tau - a).abs())
            .sum()
    }

    /// Two pieces of `region`: both grow from their own seed across the dual
    /// graph, with a step cost scaled by how far a face's normal is from the
    /// piece's running mean. Faces sharing a piece's plane cost almost nothing,
    /// so a flat wall stays whole and the cut settles along the bends.
    fn split(&mut self, region: &Region, balanced: bool) -> Vec<Region> {
        let topology = self.topology;
        let depth = region.depth + 1;
        let atoms = &region.atoms;
        if atoms.len() < 2 {
            return vec![Region { atoms: atoms.clone(), depth }];
        }
        let mark = self.next_region;
        self.next_region += 1;
        for &a in atoms {
            self.region_of[a as usize] = mark;
        }
        let seeds = self.seeds(atoms);
        if balanced {
            let half = atoms.len() / 2;
            let mut queue = std::collections::VecDeque::from([seeds[0]]);
            self.side[seeds[0] as usize] = 0;
            let mut taken = 1;
            while let Some(a) = queue.pop_front() {
                if taken >= half {
                    break;
                }
                let side = &mut self.side;
                for_neighbours(topology, &self.region_of, mark, a, |other, _| {
                    if taken < half && side[other as usize] == UNSET {
                        side[other as usize] = 0;
                        taken += 1;
                        queue.push_back(other);
                    }
                });
            }
        } else {
            let mut seed_pair = seeds;
            for round in 0..2 {
                for &a in atoms {
                    self.side[a as usize] = UNSET;
                }
                let means = self.grow(&seed_pair, mark);
                if round == 0 {
                    // Recentre each seed on the face closest to its piece's
                    // mean, the Lloyd step that lets the cut move to a bend.
                    for piece in 0..2u8 {
                        let mut best = (f64::NEG_INFINITY, seed_pair[piece as usize]);
                        for &a in atoms {
                            if self.side[a as usize] == piece {
                                let d = dot(topology.atom_normal[a as usize], means[piece as usize]);
                                if d > best.0 {
                                    best = (d, a);
                                }
                            }
                        }
                        seed_pair[piece as usize] = best.1;
                    }
                }
            }
        }
        let mut pieces = [Vec::new(), Vec::new()];
        for &a in atoms {
            // Anything growth could not reach stays with the second piece and
            // comes apart again in `components`.
            let piece = if self.side[a as usize] == 0 { 0 } else { 1 };
            pieces[piece].push(a);
            self.side[a as usize] = UNSET;
        }
        if pieces[0].is_empty() || pieces[1].is_empty() {
            let half = atoms.len() / 2;
            pieces = [atoms[..half].to_vec(), atoms[half..].to_vec()];
        }
        let [a, b] = pieces;
        let mut out = self.components(a, depth);
        out.extend(self.components(b, depth));
        out
    }

    /// The two faces with the most different normals; on a flat region, whose
    /// normals all agree, the two farthest apart in space.
    fn seeds(&self, atoms: &[u32]) -> [u32; 2] {
        let topology = self.topology;
        let mut mean = [0.0; 3];
        for &a in atoms {
            let n = topology.atom_normal[a as usize];
            let w = topology.atom_area[a as usize];
            for d in 0..3 {
                mean[d] += n[d] * w;
            }
        }
        let mean = normalized(mean);
        let extreme = |reference: [f64; 3]| {
            let mut best = (f64::INFINITY, atoms[0]);
            for &a in atoms {
                let d = dot(topology.atom_normal[a as usize], reference);
                if d < best.0 {
                    best = (d, a);
                }
            }
            best
        };
        let first = extreme(mean).1;
        let (agreement, second) = extreme(topology.atom_normal[first as usize]);
        if agreement < 0.99 && first != second {
            return [first, second];
        }
        let centre = |a: u32| topology.corner_position(topology.tris_of(a)[0] as usize * 3);
        let farthest = |from: [f64; 3]| {
            let mut best = (-1.0, atoms[0]);
            for &a in atoms {
                let d = sub(centre(a), from);
                let d = dot(d, d);
                if d > best.0 {
                    best = (d, a);
                }
            }
            best.1
        };
        let first = farthest(centre(atoms[0]));
        [first, farthest(centre(first))]
    }

    /// Fills `side` by simultaneous growth; returns each piece's mean normal.
    fn grow(&mut self, seeds: &[u32; 2], mark: u32) -> [[f64; 3]; 2] {
        let topology = self.topology;
        let mut sum = [[0.0; 3]; 2];
        let mut heap = BinaryHeap::new();
        for piece in 0..2 {
            heap.push(Reverse((Cost(0.0), seeds[piece], piece as u8)));
        }
        while let Some(Reverse((Cost(cost), atom, piece))) = heap.pop() {
            if self.side[atom as usize] != UNSET {
                continue;
            }
            self.side[atom as usize] = piece;
            let n = topology.atom_normal[atom as usize];
            let w = topology.atom_area[atom as usize];
            for d in 0..3 {
                sum[piece as usize][d] += n[d] * w;
            }
            let mean = normalized(sum[piece as usize]);
            let side = &self.side;
            for_neighbours(topology, &self.region_of, mark, atom, |other, len| {
                if side[other as usize] == UNSET {
                    let deviation = 1.0 - dot(topology.atom_normal[other as usize], mean);
                    heap.push(Reverse((Cost(cost + len * (0.02 + deviation)), other, piece)));
                }
            });
        }
        [normalized(sum[0]), normalized(sum[1])]
    }
}

#[derive(PartialEq)]
struct Cost(f64);
impl Eq for Cost {}
impl PartialOrd for Cost {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cost {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0)
    }
}

/// Neighbours of `atom` inside the stamped region, with the shared edge's length.
fn for_neighbours(
    topology: &Topology<'_>,
    region_of: &[u32],
    mark: u32,
    atom: u32,
    mut visit: impl FnMut(u32, f64),
) {
    for &t in topology.tris_of(atom) {
        for k in 0..3 {
            let e = t as usize * 3 + k;
            let o = topology.opposite[e];
            if o == NONE || topology.cut[e] {
                continue;
            }
            let other = topology.atom_of[(o / 3) as usize];
            if other != atom && region_of[other as usize] == mark {
                let len = length(sub(
                    topology.corner_position(t as usize * 3 + (k + 1) % 3),
                    topology.corner_position(e),
                ));
                visit(other, len);
            }
        }
    }
}

fn plane_basis(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let abs = n.map(f64::abs);
    let helper = if abs[0] <= abs[1] && abs[0] <= abs[2] {
        [1.0, 0.0, 0.0]
    } else if abs[1] <= abs[2] {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = normalized(cross(helper, n));
    (u, cross(n, u))
}

/// Least Squares Conformal Map: minimises, per triangle, the failure of the
/// Cauchy-Riemann equations, over vertices with two boundary pins. Returns
/// `None` when cancelled.
fn conformal(
    topology: &Topology<'_>,
    tris: &[u32],
    corners: &[u32],
    positions: &[[f64; 3]],
    warm: Vec<C>,
    boundary_next: &[u32],
    checkpoint: &mut Checkpoint<'_>,
) -> Option<Vec<C>> {
    let n = positions.len();
    let (pin_a, pin_b) = pins(positions, boundary_next);
    // Warm start: the projection moved by the similarity that takes its pins
    // to their solved places. On a gentle chart that is nearly the answer.
    let span = length(sub(positions[pin_b], positions[pin_a]));
    let pa = warm[pin_a];
    let d = [warm[pin_b][0] - pa[0], warm[pin_b][1] - pa[1]];
    let dd = d[0] * d[0] + d[1] * d[1];
    let mut uv: Vec<C> = if dd > span * span * 1.0e-6 {
        let k = [d[0] * span / dd, -d[1] * span / dd];
        warm.iter()
            .map(|p| {
                let q = [p[0] - pa[0], p[1] - pa[1]];
                [k[0] * q[0] - k[1] * q[1], k[0] * q[1] + k[1] * q[0]]
            })
            .collect()
    } else {
        vec![[0.0, 0.0]; n]
    };
    uv[pin_a] = [0.0, 0.0];
    uv[pin_b] = [span, 0.0];

    // Unknowns skip the pins.
    let mut unknown = vec![NONE; n];
    let mut count = 0u32;
    for v in 0..n {
        if v != pin_a && v != pin_b {
            unknown[v] = count;
            count += 1;
        }
    }
    let count = count as usize;
    if count == 0 {
        return Some(uv);
    }
    let floor = {
        let total: f64 = tris.iter().map(|&t| topology.area[t as usize]).sum();
        (total / tris.len() as f64 * 1.0e-6).max(f64::MIN_POSITIVE)
    };
    let mut triplets: Vec<(u32, u32, C)> = Vec::with_capacity(tris.len() * 9);
    let mut rhs = vec![[0.0; 2]; count];
    for (i, &t) in tris.iter().enumerate() {
        let tri = [corners[i * 3], corners[i * 3 + 1], corners[i * 3 + 2]];
        let p = tri.map(|v| positions[v as usize]);
        // The triangle in its own plane: first corner at the origin, second
        // on the x axis.
        let e1 = sub(p[1], p[0]);
        let e2 = sub(p[2], p[0]);
        let x1 = length(e1);
        if !(x1 > 0.0) {
            continue;
        }
        let x2 = dot(e2, e1) / x1;
        let y2 = length(cross(e1, e2)) / x1;
        let local = [[0.0, 0.0], [x1, 0.0], [x2, y2]];
        let w: [C; 3] = std::array::from_fn(|k| {
            let (a, b) = (local[(k + 2) % 3], local[(k + 1) % 3]);
            [a[0] - b[0], a[1] - b[1]]
        });
        let weight = 1.0 / (2.0 * topology.area[t as usize].max(floor));
        for a in 0..3 {
            let ua = unknown[tri[a] as usize];
            if ua == NONE {
                continue;
            }
            for b in 0..3 {
                // conj(w[a]) * w[b]
                let value = [
                    (w[a][0] * w[b][0] + w[a][1] * w[b][1]) * weight,
                    (w[a][0] * w[b][1] - w[a][1] * w[b][0]) * weight,
                ];
                let ub = unknown[tri[b] as usize];
                if ub != NONE {
                    triplets.push((ua, ub, value));
                } else {
                    let pinned = uv[tri[b] as usize];
                    let slot = &mut rhs[ua as usize];
                    slot[0] -= value[0] * pinned[0] - value[1] * pinned[1];
                    slot[1] -= value[0] * pinned[1] + value[1] * pinned[0];
                }
            }
        }
    }
    // Every free vertex needs a row even when its triangles had no equation.
    for u in 0..count as u32 {
        triplets.push((u, u, [0.0, 0.0]));
    }
    let matrix = Csr::from_triplets(count, &triplets);
    drop(triplets);
    let mut x = vec![[0.0; 2]; count];
    for v in 0..n {
        if unknown[v] != NONE {
            x[unknown[v] as usize] = uv[v];
        }
    }
    if !solve::solve(matrix, &rhs, &mut x, checkpoint) {
        return None;
    }
    for v in 0..n {
        if unknown[v] != NONE {
            uv[v] = x[unknown[v] as usize];
        }
    }
    Some(uv)
}

/// Two boundary vertices far apart: the double sweep over the boundary.
fn pins(positions: &[[f64; 3]], boundary_next: &[u32]) -> (usize, usize) {
    let boundary: Vec<usize> =
        (0..positions.len()).filter(|&v| boundary_next[v] != NONE).collect();
    let pool: Vec<usize> = if boundary.len() >= 2 {
        boundary
    } else {
        (0..positions.len()).collect()
    };
    let farthest = |from: usize| {
        let mut best = (-1.0, from);
        for &v in &pool {
            let d = sub(positions[v], positions[from]);
            let d = dot(d, d);
            if d > best.0 {
                best = (d, v);
            }
        }
        best.1
    };
    let a = farthest(pool[0]);
    let b = farthest(a);
    if a != b {
        (a, b)
    } else {
        (pool[0], *pool.iter().find(|&&v| v != pool[0]).unwrap_or(&pool[0]))
    }
}

/// Scales `uv` to the chart's true area. Returns the area-weighted quantile of
/// the worst-direction stretch and whether any real triangle folded over.
fn normalize_and_measure(
    topology: &Topology<'_>,
    tris: &[u32],
    corners: &[u32],
    positions: &[[f64; 3]],
    uv: &mut [C],
) -> (f64, bool) {
    let signed = |tri: &[u32], uv: &[C]| {
        let (a, b, c) = (uv[tri[0] as usize], uv[tri[1] as usize], uv[tri[2] as usize]);
        ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) * 0.5
    };
    let mut area_3d = 0.0;
    let mut area_uv = 0.0;
    for (i, tri) in corners.chunks_exact(3).enumerate() {
        let t = tris[i] as usize;
        if !topology.sliver[t] {
            area_3d += topology.area[t];
            area_uv += signed(tri, uv).abs();
        }
    }
    if area_uv > 0.0 && area_3d > 0.0 {
        let scale = (area_3d / area_uv).sqrt();
        for p in uv.iter_mut() {
            p[0] *= scale;
            p[1] *= scale;
        }
    }
    let mut flipped = false;
    let mut samples: Vec<(f64, f64)> = Vec::with_capacity(tris.len());
    for (i, tri) in corners.chunks_exact(3).enumerate() {
        let t = tris[i] as usize;
        if topology.sliver[t] {
            continue;
        }
        let a = topology.area[t];
        let flat_area = signed(tri, uv);
        if flat_area <= a * 1.0e-9 {
            flipped = true;
        }
        let stretch = triangle_stretch(
            [positions[tri[0] as usize], positions[tri[1] as usize], positions[tri[2] as usize]],
            [uv[tri[0] as usize], uv[tri[1] as usize], uv[tri[2] as usize]],
        );
        samples.push((stretch, a));
    }
    samples.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut remaining = samples.iter().map(|s| s.1).sum::<f64>() * STRETCH_QUANTILE;
    let mut quantile = 1.0;
    for s in &samples {
        quantile = s.0;
        remaining -= s.1;
        if remaining <= 0.0 {
            break;
        }
    }
    (quantile, flipped)
}

/// Singular values of a triangle's map from the surface to uv: `(larger,
/// smaller)`. Both 1 means an isometry. Their ratio measures angle distortion
/// and their product area distortion.
pub fn triangle_singular_values(p: [[f64; 3]; 3], uv: [[f64; 2]; 3]) -> (f64, f64) {
    let e1 = sub(p[1], p[0]);
    let e2 = sub(p[2], p[0]);
    let x1 = length(e1);
    if !(x1 > 0.0) {
        return (1.0, 1.0);
    }
    let x2 = dot(e2, e1) / x1;
    let y2 = length(cross(e1, e2)) / x1;
    if !(y2 > 0.0) {
        return (1.0, 1.0);
    }
    let d1 = [uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]];
    let d2 = [uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]];
    let jx = [d1[0] / x1, d1[1] / x1];
    let jy = [(d2[0] - jx[0] * x2) / y2, (d2[1] - jx[1] * x2) / y2];
    let e = jx[0] * jx[0] + jx[1] * jx[1];
    let g = jy[0] * jy[0] + jy[1] * jy[1];
    let f = jx[0] * jy[0] + jx[1] * jy[1];
    let root = ((e - g) * (e - g) + 4.0 * f * f).sqrt();
    (((e + g + root) * 0.5).sqrt(), ((e + g - root).max(0.0) * 0.5).sqrt())
}

fn triangle_stretch(p: [[f64; 3]; 3], uv: [[f64; 2]; 3]) -> f64 {
    let (large, small) = triangle_singular_values(p, uv);
    large.max(if small > 0.0 { 1.0 / small } else { f64::INFINITY })
}

/// True when two boundary edges of the chart properly cross in uv. With no
/// flipped triangle that is the only way a flattening can overlap itself.
fn boundary_crosses(
    uv: &[C],
    tris: &[u32],
    corners: &[u32],
    is_boundary: impl Fn(usize, usize) -> bool,
) -> bool {
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for i in 0..tris.len() {
        for k in 0..3 {
            if is_boundary(i, k) {
                edges.push((corners[i * 3 + k], corners[i * 3 + (k + 1) % 3]));
            }
        }
    }
    if edges.len() < 4 {
        return false;
    }
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for &(a, _) in &edges {
        for d in 0..2 {
            low[d] = low[d].min(uv[a as usize][d]);
            high[d] = high[d].max(uv[a as usize][d]);
        }
    }
    let side = ((edges.len() as f64).sqrt() as usize).clamp(1, 1024);
    let extent = [(high[0] - low[0]).max(1.0e-300), (high[1] - low[1]).max(1.0e-300)];
    let cell_of = |p: C, d: usize| {
        (((p[d] - low[d]) / extent[d] * side as f64) as usize).min(side - 1)
    };
    let mut entries: Vec<(u32, u32)> = Vec::with_capacity(edges.len() * 2);
    for (index, &(a, b)) in edges.iter().enumerate() {
        let (pa, pb) = (uv[a as usize], uv[b as usize]);
        let (x0, x1) = (cell_of(pa, 0).min(cell_of(pb, 0)), cell_of(pa, 0).max(cell_of(pb, 0)));
        let (y0, y1) = (cell_of(pa, 1).min(cell_of(pb, 1)), cell_of(pa, 1).max(cell_of(pb, 1)));
        for y in y0..=y1 {
            for x in x0..=x1 {
                entries.push(((y * side + x) as u32, index as u32));
            }
        }
    }
    entries.sort_unstable();
    let orient = |a: C, b: C, c: C| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let mut at = 0;
    while at < entries.len() {
        let cell = entries[at].0;
        let mut end = at + 1;
        while end < entries.len() && entries[end].0 == cell {
            end += 1;
        }
        for i in at..end {
            let (a, b) = edges[entries[i].1 as usize];
            for j in i + 1..end {
                let (c, d) = edges[entries[j].1 as usize];
                if a == c || a == d || b == c || b == d {
                    continue;
                }
                let (pa, pb, pc, pd) = (uv[a as usize], uv[b as usize], uv[c as usize], uv[d as usize]);
                let o1 = orient(pa, pb, pc);
                let o2 = orient(pa, pb, pd);
                let o3 = orient(pc, pd, pa);
                let o4 = orient(pc, pd, pb);
                if (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0) && o1 != 0.0 && o2 != 0.0 && o3 != 0.0 && o4 != 0.0 {
                    return true;
                }
            }
        }
        at = end;
    }
    false
}
