//! The acceleration structure: a binned-SAH BVH2 flattened into a THREADED
//! (skip-link) layout, traversed with no stack and no local arrays.
//!
//! Why threaded: it makes bounded-stack overflow structurally impossible on
//! every backend. A threaded BVH visits nodes in DFS order: a HIT of an interior box falls through to
//! the next node (`i + 1` = its first child), a MISS jumps the whole
//! subtree via the node's `skip` link. Leaves test their triangles inline
//! and continue at `i + 1` either way. One `u32` of state, zero arrays.
//!
//! Texels (2 per node):
//!
//! ```text
//! T0 = (bmin.x, bmin.y, bmin.z, code)   code < 0: leaf (see below), else interior
//! T1 = (bmax.x, bmax.y, bmax.z, skip)   skip = next DFS index if the box misses
//! ```
//!
//! A leaf stores `code = -count` and `skip = first`. Interior nodes store a
//! non-negative code and their DFS skip link in `skip`. Keeping `first` as a
//! plain f32 integer, instead of packing `first << 3 | count`, extends the
//! exact encoding from 2^21 to the f32 integer limit of 2^24 triangles.
//!
//! Traversal here (`Bvh::trace`) is the CPU twin of the shader's loop —
//! same order, same watertight Woop intersection — used by the tests, the
//! CPU reference integrator and click-to-focus.

use makepad_draw::*;

/// Hard bound on nodes visited per ray, on the GPU and here. A ray that
/// reaches this cap is marked invalid rather than silently reported as a
/// miss. The integrator rejects that sample, so the safety bound cannot
/// introduce missing geometry or light leaks.
///
/// Sized from measurement, not hope: building models stack coincident
/// construction layers, which the SAH cannot separate, so grazing rays
/// along a roof line legitimately visit over a thousand nodes (woodside,
/// 50k tris, default camera: p50 373, p99 1163, max 1729 visited nodes).
/// At 1024 nearly 3% of primary rays truncated — a permanent black hole in
/// the valley of the roof and behind every window. 2048 covers the measured
/// maximum with headroom and stays a bounded, watchdog-safe loop.
pub const MAX_STEPS: u32 = 2048;

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: Vec3f,
    max: Vec3f,
}

impl Aabb {
    fn empty() -> Self {
        Self {
            min: vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY),
            max: vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
        }
    }
    fn grow(&mut self, p: Vec3f) {
        self.min = vec3f(self.min.x.min(p.x), self.min.y.min(p.y), self.min.z.min(p.z));
        self.max = vec3f(self.max.x.max(p.x), self.max.y.max(p.y), self.max.z.max(p.z));
    }
    fn union(&mut self, o: &Aabb) {
        self.grow(o.min);
        self.grow(o.max);
    }
    fn area(&self) -> f32 {
        if self.max.x < self.min.x {
            return 0.0;
        }
        let d = self.max - self.min;
        2.0 * (d.x * d.y + d.y * d.z + d.z * d.x)
    }
    fn axis(&self, a: usize) -> (f32, f32) {
        match a {
            0 => (self.min.x, self.max.x),
            1 => (self.min.y, self.max.y),
            _ => (self.min.z, self.max.z),
        }
    }
}

#[derive(Clone, Debug)]
enum Node2 {
    Leaf { bounds: Aabb, first: u32, count: u32 },
    Inner { bounds: Aabb, left: u32, right: u32 },
}

impl Node2 {
    fn bounds(&self) -> &Aabb {
        match self {
            Node2::Leaf { bounds, .. } | Node2::Inner { bounds, .. } => bounds,
        }
    }
}

/// One flattened, threaded node (see module docs).
#[derive(Clone, Copy, Debug)]
pub struct FlatNode {
    pub min: [f32; 3],
    /// < 0: leaf code; >= 0: interior.
    pub code: f32,
    pub max: [f32; 3],
    /// Interior: DFS index to jump to on miss. Leaf: first triangle index.
    pub skip: f32,
}

/// Leaf code helper (shared with the shader's decode).
pub fn leaf_code(count: u32) -> f32 {
    debug_assert!((1..=8).contains(&count));
    -(count as f32)
}

/// Triangle vertices in BVH order, ready for intersection.
#[derive(Clone, Debug)]
pub struct Tri {
    pub v0: Vec3f,
    pub v1: Vec3f,
    pub v2: Vec3f,
}

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    pub nodes: Vec<FlatNode>,
    /// NEW triangle index → ORIGINAL triangle index.
    pub tri_order: Vec<u32>,
    /// Triangles in NEW order.
    pub tris: Vec<Tri>,
    /// Render-only coplanar priority in NEW/BVH order.
    pub priorities: Vec<u16>,
    /// Coplanar conflict group in NEW/BVH order; zero disables tie-breaking.
    pub coplanar_groups: Vec<u32>,
    pub max_depth: u32,
    pub leaf_tris: u32,
}

/// A ray hit: `t`, the NEW triangle index, barycentrics (u, v) of v1/v2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub tri: i32,
    pub u: f32,
    pub v: f32,
    /// The traversal safety bound was exhausted. This is not a miss.
    pub truncated: bool,
}

impl Hit {
    pub fn miss(tmax: f32) -> Self {
        Self { t: tmax, tri: -1, u: 0.0, v: 0.0, truncated: false }
    }
    pub fn is_hit(&self) -> bool {
        self.tri >= 0
    }
}

/// Per-ray constants for Woop's watertight ray/triangle test.
#[derive(Clone, Copy, Debug)]
pub struct RayPrep {
    pub ro: Vec3f,
    pub rd: Vec3f,
    pub inv: Vec3f,
    /// Unit masks selecting the shear axes (kx, ky, kz) by dot product —
    /// the shader has no dynamic vector index, so the CPU twin does it the
    /// same way.
    pub mx: Vec3f,
    pub my: Vec3f,
    pub mz: Vec3f,
    pub sx: f32,
    pub sy: f32,
    pub sz: f32,
}

impl RayPrep {
    pub fn new(ro: Vec3f, rd: Vec3f) -> Self {
        // Zero components: nudge only the reciprocal used by slabs. Keep the
        // actual direction for Woop's shear so the geometric ray is unchanged.
        let fix = |c: f32| if c.abs() < 1.0e-9 { if c < 0.0 { -1.0e-9 } else { 1.0e-9 } } else { c };
        let safe_rd = vec3f(fix(rd.x), fix(rd.y), fix(rd.z));
        let ax = rd.x.abs();
        let ay = rd.y.abs();
        let az = rd.z.abs();
        let (mx, my, mz) = if az >= ax && az >= ay {
            (vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0))
        } else if ax >= ay {
            (vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), vec3f(1.0, 0.0, 0.0))
        } else {
            (vec3f(0.0, 0.0, 1.0), vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0))
        };
        let dz = rd.dot(mz);
        Self {
            ro,
            rd,
            inv: vec3f(1.0 / safe_rd.x, 1.0 / safe_rd.y, 1.0 / safe_rd.z),
            mx,
            my,
            mz,
            sx: rd.dot(mx) / dz,
            sy: rd.dot(my) / dz,
            sz: 1.0 / dz,
        }
    }

    /// Woop et al. 2013, "Watertight Ray/Triangle Intersection". Returns
    /// (t, u, v, front) or None. Two-sided; `front` is true when the
    /// triangle's geometric normal faces the ray origin (`det * sz > 0`:
    /// `det` carries the projected winding along the dominant axis and `sz`
    /// its sign relative to the ray, so the product is the facing — the GPU
    /// twin computes the identical product).
    #[inline]
    pub fn intersect(&self, tri: &Tri, tmax: f32) -> Option<(f32, f32, f32, bool)> {
        let a = tri.v0 - self.ro;
        let b = tri.v1 - self.ro;
        let c = tri.v2 - self.ro;
        let (az, bz, cz) = (a.dot(self.mz), b.dot(self.mz), c.dot(self.mz));
        let ax = a.dot(self.mx) - self.sx * az;
        let ay = a.dot(self.my) - self.sy * az;
        let bx = b.dot(self.mx) - self.sx * bz;
        let by = b.dot(self.my) - self.sy * bz;
        let cx = c.dot(self.mx) - self.sx * cz;
        let cy = c.dot(self.my) - self.sy * cz;
        let u = cx * by - cy * bx;
        let v = ax * cy - ay * cx;
        let w = bx * ay - by * ax;
        if (u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0) {
            return None;
        }
        let det = u + v + w;
        if det == 0.0 {
            return None;
        }
        let t = (u * self.sz * az + v * self.sz * bz + w * self.sz * cz) / det;
        if t <= 0.0 || t > tmax {
            return None;
        }
        Some((t, v / det, w / det, det * self.sz > 0.0))
    }
}

/// Building geometry is authored in metres. Two hits closer than the scene's
/// 2 mm coplanar analysis tolerance (plus a small relative f32 term) are a
/// depth tie when either triangle carries a non-zero overlap priority.
#[inline]
pub fn hit_tie_epsilon(t: f32) -> f32 {
    0.002 + t.abs() * 2.0e-6
}

#[inline]
fn candidate_wins(
    t: f32,
    front: bool,
    priority: u16,
    group: u32,
    original: u32,
    hit: &Hit,
    hit_front: bool,
    hit_priority: u16,
    hit_group: u32,
    hit_original: u32,
) -> bool {
    if !hit.is_hit() {
        return true;
    }
    let prioritized = group != 0 && group == hit_group;
    if prioritized && (t - hit.t).abs() <= hit_tie_epsilon(t.max(hit.t)) {
        // Facing outranks priority inside a coplanar tie: stacked
        // construction layers carry priorities for the EXPOSED side, and a
        // face turned away from the viewer is never the exposed one.
        // Measured on the woodside roof: framing boards are front/back pairs
        // 0.1..0.4 mm apart, and priority-first deterministically picked the
        // farther back face on alternating pixels — the black stripe of the
        // pair of stripes the roof drew.
        if front != hit_front {
            return front;
        }
        if priority != hit_priority {
            return priority > hit_priority;
        }
        if t != hit.t {
            return t < hit.t;
        }
        return original < hit_original;
    }
    t < hit.t || (t == hit.t && original < hit_original)
}

impl Bvh {
    /// Build over `tris` (original order).
    pub fn build(tris: &[Tri]) -> Bvh {
        Self::build_with_coplanar(tris, &[], &[])
    }

    /// Build with one render-only coplanar priority per original triangle.
    pub fn build_with_priorities(tris: &[Tri], priorities: &[u16]) -> Bvh {
        let groups: Vec<u32> = priorities
            .iter()
            .map(|&priority| if priority == 0 { 0 } else { 1 })
            .collect();
        Self::build_with_coplanar(tris, priorities, &groups)
    }

    /// Build with render-only priority and measured conflict group per
    /// original triangle. Priority comparisons are only valid within a group.
    pub fn build_with_coplanar(tris: &[Tri], priorities: &[u16], groups: &[u32]) -> Bvh {
        let n = tris.len();
        if n == 0 {
            return Bvh::default();
        }
        let boxes: Vec<Aabb> = tris
            .iter()
            .map(|t| {
                let mut b = Aabb::empty();
                b.grow(t.v0);
                b.grow(t.v1);
                b.grow(t.v2);
                b
            })
            .collect();
        let cents: Vec<Vec3f> = boxes.iter().map(|b| (b.min + b.max) * 0.5).collect();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes2: Vec<Node2> = Vec::with_capacity(2 * n / 3 + 1);
        let mut max_depth = 0;
        build2(&boxes, &cents, &mut order, 0, n, &mut nodes2, 0, &mut max_depth);
        // Flatten to DFS order with skip links.
        let mut nodes = Vec::with_capacity(nodes2.len());
        flatten(&nodes2, 0, &mut nodes);
        let tris_new: Vec<Tri> = order.iter().map(|&i| tris[i as usize].clone()).collect();
        let priorities_new: Vec<u16> = order
            .iter()
            .map(|&i| priorities.get(i as usize).copied().unwrap_or(0))
            .collect();
        let groups_new: Vec<u32> = order
            .iter()
            .map(|&i| groups.get(i as usize).copied().unwrap_or(0))
            .collect();
        let leaf_tris = nodes
            .iter()
            .filter(|nd| nd.code < 0.0)
            .map(|nd| (-nd.code) as u32)
            .sum();
        Bvh {
            nodes,
            tri_order: order,
            tris: tris_new,
            priorities: priorities_new,
            coplanar_groups: groups_new,
            max_depth,
            leaf_tris,
        }
    }

    /// Pack the nodes into texels (2 per node, 4 floats each).
    pub fn texels(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.nodes.len() * 8);
        for nd in &self.nodes {
            out.extend([nd.min[0], nd.min[1], nd.min[2], nd.code]);
            out.extend([nd.max[0], nd.max[1], nd.max[2], nd.skip]);
        }
        out
    }

    /// Closest hit (`any_hit = false`) or first hit found (`any_hit = true`).
    pub fn trace(&self, ro: Vec3f, rd: Vec3f, tmax: f32, any_hit: bool) -> Hit {
        self.trace_from(ro, rd, 0.0, tmax, any_hit)
    }

    /// `trace` with a near limit: hits closer than `tmin` are ignored, which
    /// is how secondary rays step off coincident or self faces (legacy BIM
    /// meshes overlap and are unwelded). The exact loop the shader runs,
    /// including its `MAX_STEPS` bound.
    pub fn trace_from(&self, ro: Vec3f, rd: Vec3f, tmin: f32, tmax: f32, any_hit: bool) -> Hit {
        self.trace_skip(ro, rd, tmin, tmax, any_hit, -1)
    }

    /// Like `trace_from`, but the BVH-order triangle `skip` (≥ 0) is never
    /// returned. Secondary and shadow rays pass the originating triangle so
    /// a t≈0 self-hit cannot win the watertight tie-break over a neighbour.
    pub fn trace_skip(
        &self,
        ro: Vec3f,
        rd: Vec3f,
        tmin: f32,
        tmax: f32,
        any_hit: bool,
        skip: i32,
    ) -> Hit {
        self.trace_from_limit(ro, rd, tmin, tmax, any_hit, MAX_STEPS, skip).0
    }

    fn trace_from_limit(
        &self,
        ro: Vec3f,
        rd: Vec3f,
        tmin: f32,
        tmax: f32,
        any_hit: bool,
        max_steps: u32,
        skip: i32,
    ) -> (Hit, u32) {
        let mut hit = Hit::miss(tmax);
        let count = self.nodes.len() as u32;
        if count == 0 {
            return (hit, 0);
        }
        let ray = RayPrep::new(ro, rd);
        let mut i = 0u32;
        let mut steps = 0u32;
        let mut hit_front = false;
        let mut hit_priority = 0u16;
        let mut hit_group = 0u32;
        let mut hit_original = u32::MAX;
        while i < count && steps < max_steps {
            steps += 1;
            let nd = &self.nodes[i as usize];
            let t0x = (nd.min[0] - ray.ro.x) * ray.inv.x;
            let t1x = (nd.max[0] - ray.ro.x) * ray.inv.x;
            let t0y = (nd.min[1] - ray.ro.y) * ray.inv.y;
            let t1y = (nd.max[1] - ray.ro.y) * ray.inv.y;
            let t0z = (nd.min[2] - ray.ro.z) * ray.inv.z;
            let t1z = (nd.max[2] - ray.ro.z) * ray.inv.z;
            let near = t0x.min(t1x).max(t0y.min(t1y)).max(t0z.min(t1z).max(tmin));
            // Conservative WBW slab expansion: 2*gamma(3), rounded up.
            let hit_limit = if !any_hit && hit.is_hit() && hit_group != 0 {
                (hit.t + hit_tie_epsilon(hit.t)).min(tmax)
            } else {
                hit.t
            };
            let mut far = t0x
                .max(t1x)
                .min(t0y.max(t1y))
                .min(t0z.max(t1z).min(hit_limit));
            if far >= 0.0 {
                far *= 1.000_000_72;
            }
            if near <= far {
                if nd.code < 0.0 {
                    let first = nd.skip as u32;
                    let cnt = (-nd.code) as u32;
                    for k in first..first + cnt {
                        if skip >= 0 && k as i32 == skip {
                            continue;
                        }
                        let priority = self.priorities.get(k as usize).copied().unwrap_or(0);
                        let group = self.coplanar_groups.get(k as usize).copied().unwrap_or(0);
                        let tri_limit = if !any_hit && hit.is_hit() && group != 0 && group == hit_group {
                            (hit.t + hit_tie_epsilon(hit.t)).min(tmax)
                        } else {
                            hit.t
                        };
                        if let Some((t, u, v, front)) = ray.intersect(&self.tris[k as usize], tri_limit) {
                            if t <= tmin || t > tmax {
                                continue;
                            }
                            let original = self.tri_order[k as usize];
                            if candidate_wins(
                                t,
                                front,
                                priority,
                                group,
                                original,
                                &hit,
                                hit_front,
                                hit_priority,
                                hit_group,
                                hit_original,
                            ) {
                                hit = Hit { t, tri: k as i32, u, v, truncated: false };
                                hit_front = front;
                                hit_priority = priority;
                                hit_group = group;
                                hit_original = original;
                            }
                            if any_hit {
                                return (hit, steps);
                            }
                        }
                    }
                }
                i += 1;
            } else {
                i = if nd.code < 0.0 { i + 1 } else { nd.skip as u32 };
            }
        }
        if i < count {
            hit.truncated = true;
        }
        (hit, steps)
    }

    /// Diagnostics: `trace` with a caller-supplied step cap, reporting the
    /// nodes actually visited. Lets a harness histogram traversal cost on a
    /// real model instead of guessing at `MAX_STEPS`.
    pub fn trace_counted(&self, ro: Vec3f, rd: Vec3f, tmax: f32, max_steps: u32) -> (Hit, u32) {
        self.trace_from_limit(ro, rd, 0.0, tmax, false, max_steps, -1)
    }

    /// Brute force over every triangle (the oracle for the traversal test).
    pub fn trace_brute(&self, ro: Vec3f, rd: Vec3f, tmax: f32) -> Hit {
        let ray = RayPrep::new(ro, rd);
        let mut hit = Hit::miss(tmax);
        let mut hit_front = false;
        let mut hit_priority = 0u16;
        let mut hit_group = 0u32;
        let mut hit_original = u32::MAX;
        for (i, tri) in self.tris.iter().enumerate() {
            let priority = self.priorities.get(i).copied().unwrap_or(0);
            let group = self.coplanar_groups.get(i).copied().unwrap_or(0);
            let limit = if hit.is_hit() && group != 0 && group == hit_group {
                (hit.t + hit_tie_epsilon(hit.t)).min(tmax)
            } else {
                hit.t
            };
            if let Some((t, u, v, front)) = ray.intersect(tri, limit) {
                let original = self.tri_order[i];
                if candidate_wins(
                    t,
                    front,
                    priority,
                    group,
                    original,
                    &hit,
                    hit_front,
                    hit_priority,
                    hit_group,
                    hit_original,
                ) {
                    hit = Hit { t, tri: i as i32, u, v, truncated: false };
                    hit_front = front;
                    hit_priority = priority;
                    hit_group = group;
                    hit_original = original;
                }
            }
        }
        hit
    }
}

/// Emit `n2`'s subtree in DFS order; a node's `skip` is wherever the DFS
/// lands after the whole subtree.
fn flatten(nodes2: &[Node2], n2: u32, out: &mut Vec<FlatNode>) {
    let at = out.len();
    let b = *nodes2[n2 as usize].bounds();
    out.push(FlatNode { min: [b.min.x, b.min.y, b.min.z], code: 0.0, max: [b.max.x, b.max.y, b.max.z], skip: 0.0 });
    match &nodes2[n2 as usize] {
        Node2::Leaf { first, count, .. } => {
            out[at].code = leaf_code(*count);
            out[at].skip = *first as f32;
        }
        Node2::Inner { left, right, .. } => {
            out[at].code = 1.0;
            let (left, right) = (*left, *right);
            flatten(nodes2, left, out);
            flatten(nodes2, right, out);
        }
    }
    if out[at].code >= 0.0 {
        out[at].skip = out.len() as f32;
    }
}

const BINS: usize = 12;
const LEAF_MAX: usize = 4;
const MAX_DEPTH2: u32 = 40;

#[allow(clippy::too_many_arguments)]
fn build2(
    boxes: &[Aabb],
    cents: &[Vec3f],
    order: &mut [u32],
    start: usize,
    end: usize,
    nodes: &mut Vec<Node2>,
    depth: u32,
    max_depth: &mut u32,
) -> u32 {
    *max_depth = (*max_depth).max(depth);
    let mut bounds = Aabb::empty();
    let mut cbounds = Aabb::empty();
    for &i in &order[start..end] {
        bounds.union(&boxes[i as usize]);
        cbounds.grow(cents[i as usize]);
    }
    let count = end - start;
    if count <= LEAF_MAX || depth >= MAX_DEPTH2 {
        return leaf_chain(nodes, start, end, bounds, order, boxes);
    }
    // Binned SAH over the widest centroid axis.
    let ext = cbounds.max - cbounds.min;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    let (cmin, cmax) = cbounds.axis(axis);
    if cmax - cmin < 1.0e-9 {
        // All centroids coincide: median split keeps the tree finite.
        let mid = (start + end) / 2;
        let id = nodes.len() as u32;
        nodes.push(Node2::Inner { bounds, left: 0, right: 0 });
        let l = build2(boxes, cents, order, start, mid, nodes, depth + 1, max_depth);
        let r = build2(boxes, cents, order, mid, end, nodes, depth + 1, max_depth);
        nodes[id as usize] = Node2::Inner { bounds, left: l, right: r };
        return id;
    }
    let scale = BINS as f32 / (cmax - cmin);
    let bin_of = |i: u32| -> usize {
        let c = match axis {
            0 => cents[i as usize].x,
            1 => cents[i as usize].y,
            _ => cents[i as usize].z,
        };
        (((c - cmin) * scale) as usize).min(BINS - 1)
    };
    let mut bin_box = [Aabb::empty(); BINS];
    let mut bin_cnt = [0usize; BINS];
    for &i in &order[start..end] {
        let b = bin_of(i);
        bin_box[b].union(&boxes[i as usize]);
        bin_cnt[b] += 1;
    }
    // Sweep from the right for suffix costs, then from the left.
    let mut right_area = [0.0f32; BINS];
    let mut right_cnt = [0usize; BINS];
    let mut acc = Aabb::empty();
    let mut cnt = 0;
    for b in (1..BINS).rev() {
        acc.union(&bin_box[b]);
        cnt += bin_cnt[b];
        right_area[b] = acc.area();
        right_cnt[b] = cnt;
    }
    let mut best = (f32::INFINITY, 0usize);
    let mut acc = Aabb::empty();
    let mut cnt = 0;
    for b in 0..BINS - 1 {
        acc.union(&bin_box[b]);
        cnt += bin_cnt[b];
        if cnt == 0 || right_cnt[b + 1] == 0 {
            continue;
        }
        let cost = acc.area() * cnt as f32 + right_area[b + 1] * right_cnt[b + 1] as f32;
        if cost < best.0 {
            best = (cost, b);
        }
    }
    let leaf_cost = bounds.area() * count as f32;
    if best.0 == f32::INFINITY || (best.0 >= leaf_cost && count <= 8) {
        return leaf_chain(nodes, start, end, bounds, order, boxes);
    }
    // Partition.
    let split_bin = best.1;
    let mut lo = start;
    let mut hi = end;
    while lo < hi {
        if bin_of(order[lo]) <= split_bin {
            lo += 1;
        } else {
            hi -= 1;
            order.swap(lo, hi);
        }
    }
    let mid = lo;
    if mid == start || mid == end {
        return leaf_chain(nodes, start, end, bounds, order, boxes);
    }
    let id = nodes.len() as u32;
    nodes.push(Node2::Inner { bounds, left: 0, right: 0 });
    let l = build2(boxes, cents, order, start, mid, nodes, depth + 1, max_depth);
    let r = build2(boxes, cents, order, mid, end, nodes, depth + 1, max_depth);
    nodes[id as usize] = Node2::Inner { bounds, left: l, right: r };
    id
}

/// Chop a forced leaf range into leaves of ≤ 8 (depth cap fallback).
fn leaf_chain(
    nodes: &mut Vec<Node2>,
    s: usize,
    e: usize,
    bounds: Aabb,
    order: &[u32],
    boxes: &[Aabb],
) -> u32 {
    if e - s <= 8 {
        nodes.push(Node2::Leaf { bounds, first: s as u32, count: (e - s) as u32 });
        return (nodes.len() - 1) as u32;
    }
    let id = nodes.len() as u32;
    nodes.push(Node2::Inner { bounds, left: 0, right: 0 });
    let mid = (s + e) / 2;
    let mut lb = Aabb::empty();
    let mut rb = Aabb::empty();
    for &i in &order[s..mid] {
        lb.union(&boxes[i as usize]);
    }
    for &i in &order[mid..e] {
        rb.union(&boxes[i as usize]);
    }
    let l = leaf_chain(nodes, s, mid, lb, order, boxes);
    let r = leaf_chain(nodes, mid, e, rb, order, boxes);
    nodes[id as usize] = Node2::Inner { bounds, left: l, right: r };
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f32 / 16777216.0
    }

    fn random_tris(n: usize, seed: u32) -> Vec<Tri> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                let c = vec3f(lcg(&mut s) * 10.0 - 5.0, lcg(&mut s) * 10.0 - 5.0, lcg(&mut s) * 10.0 - 5.0);
                let r = |s: &mut u32| vec3f(lcg(s) - 0.5, lcg(s) - 0.5, lcg(s) - 0.5) * 0.8;
                Tri { v0: c + r(&mut s), v1: c + r(&mut s), v2: c + r(&mut s) }
            })
            .collect()
    }

    #[test]
    fn traversal_matches_brute_force_on_random_triangles() {
        let tris = random_tris(3000, 7);
        let bvh = Bvh::build(&tris);
        assert_eq!(bvh.leaf_tris as usize, tris.len(), "every triangle lands in exactly one leaf");
        assert!(bvh.max_depth < 40, "depth {}", bvh.max_depth);
        // Skip links are self-consistent: every skip lands after the node.
        for (i, nd) in bvh.nodes.iter().enumerate() {
            if nd.code >= 0.0 {
                assert!(nd.skip as usize > i && nd.skip as usize <= bvh.nodes.len());
            }
        }
        let mut s = 99u32;
        let mut hits = 0;
        for _ in 0..2000 {
            let ro = vec3f(lcg(&mut s) * 16.0 - 8.0, lcg(&mut s) * 16.0 - 8.0, lcg(&mut s) * 16.0 - 8.0);
            let rd = vec3f(lcg(&mut s) - 0.5, lcg(&mut s) - 0.5, lcg(&mut s) - 0.5).normalize();
            let a = bvh.trace(ro, rd, 1.0e30, false);
            let b = bvh.trace_brute(ro, rd, 1.0e30);
            assert_eq!(a.tri, b.tri, "closest triangle differs: {a:?} vs {b:?}");
            if a.is_hit() {
                hits += 1;
                assert!((a.t - b.t).abs() < 1.0e-5);
            }
            // any-hit: hit iff brute force hits
            let c = bvh.trace(ro, rd, 1.0e30, true);
            assert_eq!(c.is_hit(), b.is_hit());
        }
        assert!(hits > 200, "the random scene should be hit often ({hits})");
    }

    #[test]
    fn leaf_codes_round_trip() {
        for count in 1..=8u32 {
            assert_eq!((-leaf_code(count)) as u32, count);
        }
        // A leaf start remains exact well beyond the old packed-code limit.
        for first in [0u32, 1, 17, 100_000, 10_000_000, (1 << 24) - 1] {
            assert_eq!(first as f32 as u32, first);
        }
    }

    #[test]
    fn traversal_limit_is_reported_not_silently_missed() {
        let bvh = Bvh::build(&random_tris(3000, 19));
        let (h, _steps) = bvh.trace_from_limit(
            vec3f(0.0, 0.0, 20.0),
            vec3f(0.0, 0.0, -1.0),
            0.0,
            1.0e30,
            false,
            1,
            -1,
        );
        assert!(h.truncated, "a capped traversal must be diagnosed");
    }

    #[test]
    fn axis_aligned_quad_is_watertight_across_its_diagonal() {
        // Two triangles sharing a diagonal: rays through the shared edge must
        // hit exactly one of them (never fall through the crack).
        let tris = vec![
            Tri { v0: vec3f(0.0, 0.0, 0.0), v1: vec3f(1.0, 0.0, 0.0), v2: vec3f(1.0, 1.0, 0.0) },
            Tri { v0: vec3f(0.0, 0.0, 0.0), v1: vec3f(1.0, 1.0, 0.0), v2: vec3f(0.0, 1.0, 0.0) },
        ];
        let bvh = Bvh::build(&tris);
        for i in 1..200 {
            let x = i as f32 / 200.0;
            let h = bvh.trace(vec3f(x, x, 1.0), vec3f(0.0, 0.0, -1.0), 10.0, false);
            assert!(h.is_hit(), "fell through the diagonal at {x}");
            let h = bvh.trace(vec3f(x + 1.0e-7, x, 1.0), vec3f(0.001, 0.0, -1.0).normalize(), 10.0, false);
            assert!(h.is_hit(), "fell through the diagonal at {x} (oblique)");
        }
    }

    #[test]
    fn coincident_faces_choose_lowest_original_triangle_id() {
        let tri = Tri {
            v0: vec3f(-1.0, -1.0, 0.0),
            v1: vec3f(1.0, -1.0, 0.0),
            v2: vec3f(0.0, 1.0, 0.0),
        };
        let bvh = Bvh::build(&[tri.clone(), tri.clone(), tri]);
        let h = bvh.trace(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 0.0, -1.0), 10.0, false);
        assert!(h.is_hit());
        assert_eq!(bvh.tri_order[h.tri as usize], 0);
    }

    #[test]
    fn coincident_faces_choose_render_priority_not_triangle_id() {
        let tri = Tri {
            v0: vec3f(-1.0, -1.0, 0.0),
            v1: vec3f(1.0, -1.0, 0.0),
            v2: vec3f(0.0, 1.0, 0.0),
        };
        let bvh = Bvh::build_with_priorities(&[tri.clone(), tri], &[1, 9]);
        let h = bvh.trace(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 0.0, -1.0), 10.0, false);
        assert!(h.is_hit());
        assert_eq!(bvh.tri_order[h.tri as usize], 1);
    }

    #[test]
    fn priority_wins_inside_epsilon_but_not_over_real_distance() {
        let make = |z| Tri {
            v0: vec3f(-1.0, -1.0, z),
            v1: vec3f(1.0, -1.0, z),
            v2: vec3f(0.0, 1.0, z),
        };
        let near_tie = Bvh::build_with_priorities(&[make(0.0), make(-0.001)], &[1, 9]);
        let ray = (vec3f(0.0, 0.0, 1.0), vec3f(0.0, 0.0, -1.0));
        let h = near_tie.trace(ray.0, ray.1, 10.0, false);
        assert_eq!(near_tie.tri_order[h.tri as usize], 1, "priority must resolve the 1 mm tie");

        let separated = Bvh::build_with_priorities(&[make(0.0), make(-0.01)], &[1, 9]);
        let h = separated.trace(ray.0, ray.1, 10.0, false);
        assert_eq!(separated.tri_order[h.tri as usize], 0, "nearest geometry wins beyond epsilon");
    }

    #[test]
    fn facing_beats_priority_inside_a_tie() {
        // A backfacing board bottom 0.5 mm CLOSER and with HIGHER priority
        // still loses to the viewer-facing top inside the coplanar tie: the
        // exposed side of a stacked assembly is the one facing the ray.
        let front = Tri {
            v0: vec3f(-1.0, -1.0, 0.0),
            v1: vec3f(1.0, -1.0, 0.0),
            v2: vec3f(0.0, 1.0, 0.0),
        };
        let back = Tri {
            v0: vec3f(-1.0, -1.0, 0.0005),
            v1: vec3f(0.0, 1.0, 0.0005),
            v2: vec3f(1.0, -1.0, 0.0005),
        };
        let bvh = Bvh::build_with_coplanar(&[front, back], &[1, 9], &[7, 7]);
        let h = bvh.trace(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 0.0, -1.0), 10.0, false);
        assert_eq!(
            bvh.tri_order[h.tri as usize],
            0,
            "the viewer-facing face must win the tie over a closer, higher-priority backface"
        );
        // Seen from BELOW the same pair flips: what faced away now faces the
        // ray and wins, regardless of priority order.
        let h = bvh.trace(vec3f(0.0, 0.0, -1.0), vec3f(0.0, 0.0, 1.0), 10.0, false);
        assert_eq!(bvh.tri_order[h.tri as usize], 1);
    }

    #[test]
    fn intersect_facing_matches_geometric_normal_for_all_axes() {
        // front == (dot(ng, rd) < 0) for every dominant axis and ray sign,
        // for both windings. This is the CPU statement of the GPU's
        // det * sz > 0 (the identical product).
        let tris = [
            // z-dominant-normal triangle
            Tri { v0: vec3f(0.0, 0.0, 0.0), v1: vec3f(1.0, 0.0, 0.0), v2: vec3f(0.0, 1.0, 0.0) },
            // x-dominant
            Tri { v0: vec3f(0.0, 0.0, 0.0), v1: vec3f(0.0, 1.0, 0.0), v2: vec3f(0.0, 0.0, 1.0) },
            // y-dominant
            Tri { v0: vec3f(0.0, 0.0, 0.0), v1: vec3f(0.0, 0.0, 1.0), v2: vec3f(1.0, 0.0, 0.0) },
        ];
        let dirs = [
            vec3f(0.1, 0.2, -1.0),
            vec3f(0.1, 0.2, 1.0),
            vec3f(-1.0, 0.1, 0.2),
            vec3f(1.0, 0.1, 0.2),
            vec3f(0.2, -1.0, 0.1),
            vec3f(0.2, 1.0, 0.1),
        ];
        for tri in &tris {
            let flipped = Tri { v0: tri.v0, v1: tri.v2, v2: tri.v1 };
            for t in [tri, &flipped] {
                let ng = Vec3f::cross(t.v1 - t.v0, t.v2 - t.v0);
                let centre = (t.v0 + t.v1 + t.v2) / 3.0;
                for rd in &dirs {
                    let rd = rd.normalize();
                    let ro = centre - rd * 5.0;
                    let ray = RayPrep::new(ro, rd);
                    let Some((_, _, _, front)) = ray.intersect(t, 100.0) else {
                        panic!("ray through the centroid must hit");
                    };
                    assert_eq!(
                        front,
                        ng.dot(rd) < 0.0,
                        "facing mismatch for ng {ng:?} rd {rd:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn priority_never_crosses_coplanar_groups() {
        let make = |z| Tri {
            v0: vec3f(-1.0, -1.0, z),
            v1: vec3f(1.0, -1.0, z),
            v2: vec3f(0.0, 1.0, z),
        };
        let bvh = Bvh::build_with_coplanar(
            &[make(0.0), make(-0.001)],
            &[1, 9],
            &[10, 11],
        );
        let h = bvh.trace(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 0.0, -1.0), 10.0, false);
        assert_eq!(
            bvh.tri_order[h.tri as usize],
            0,
            "unrelated groups must use the true nearest surface"
        );
    }

    #[test]
    fn skip_id_excludes_the_source_triangle_from_the_tie_break() {
        let a = Tri {
            v0: vec3f(-1.0, -1.0, 0.0),
            v1: vec3f(1.0, -1.0, 0.0),
            v2: vec3f(0.0, 1.0, 0.0),
        };
        let bvh = Bvh::build(&[a.clone(), a]);
        let ro = vec3f(0.0, 0.0, 1.0);
        let rd = vec3f(0.0, 0.0, -1.0);
        let all = bvh.trace(ro, rd, 10.0, false);
        assert_eq!(bvh.tri_order[all.tri as usize], 0);
        let skipped = bvh.trace_skip(ro, rd, 0.0, 10.0, false, all.tri);
        assert!(skipped.is_hit());
        assert_ne!(skipped.tri, all.tri);
        assert_eq!(bvh.tri_order[skipped.tri as usize], 1);
    }

    #[test]
    fn first_hit_at_tmax_is_kept() {
        let tri = Tri {
            v0: vec3f(-1.0, -1.0, 0.0),
            v1: vec3f(1.0, -1.0, 0.0),
            v2: vec3f(0.0, 1.0, 0.0),
        };
        let bvh = Bvh::build(&[tri]);
        let ro = vec3f(0.0, 0.0, 1.0);
        let rd = vec3f(0.0, 0.0, -1.0);
        assert!(bvh.trace(ro, rd, 1.0, false).is_hit());
        assert!(bvh.trace_brute(ro, rd, 1.0).is_hit());
    }
}
