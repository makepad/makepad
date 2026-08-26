//! Spatial index over batch triangles: ray picking, frustum queries, snapping.
//!
//! A binned-SAH binary BVH built once on the loader thread from the merged
//! [`RenderBatch`]es. Leaves hold up to [`MAX_LEAF`] triangles; every primitive
//! carries its owning [`ElementId`] so the visibility filter is an array lookup
//! rather than a binary search per triangle.
//!
//! Alongside the triangle tree the index keeps **per-element world bounds**,
//! which is what frustum culling actually wants: element counts are in the
//! hundreds or thousands even when triangle counts are in the millions, so
//! [`Bvh::frustum_elements`] is a linear scan over those and never touches the
//! tree.
//!
//! Everything here is pure data and `Send + Sync`.

use crate::model::batch::RenderBatch;
use crate::model::bounds::{aabb_empty, aabb_is_empty, aabb_union, aabb_union_point};
use crate::model::ids::ElementId;
use crate::model::query::{Frustum, Ray, RayHit};
use makepad_math::{Aabb, Vec3f};

/// Triangles per leaf, upper bound. Small leaves cost nodes; big leaves cost
/// intersection tests. 8 is the usual sweet spot for architectural meshes.
pub const MAX_LEAF: usize = 8;
/// SAH bins per split axis.
const BINS: usize = 16;
/// Relative cost of a node traversal vs. one triangle test.
const TRAV_COST: f32 = 1.2;

#[derive(Clone, Copy, Debug)]
struct Prim {
    batch: u32,
    tri: u32,
    element: u32,
}

/// `count == 0` → interior node, `first` is the **left** child and the right
/// child is `first + 1` (children are always allocated as a pair).
/// `count > 0` → leaf over `prims[first .. first + count]`.
#[derive(Clone, Copy, Debug)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    first: u32,
    count: u32,
}

impl Node {
    fn bounds(&self) -> Aabb {
        Aabb {
            min: Vec3f {
                x: self.min[0],
                y: self.min[1],
                z: self.min[2],
            },
            max: Vec3f {
                x: self.max[0],
                y: self.max[1],
                z: self.max[2],
            },
        }
    }
}

/// How a pick should treat the scene. Built by [`crate::model::Scene::pick`] from the
/// live [`crate::model::SceneState`]; lanes can build their own for special cases
/// (measurement snapping ignores nothing, x-ray picks through, …).
pub struct PickOptions<'a> {
    /// Elements the pick may hit. Cheap: called once per candidate triangle.
    pub visible: &'a dyn Fn(ElementId) -> bool,
    /// Extra world-space test on the hit point — the section half-spaces, so a
    /// pick under an active section plane selects what is actually on screen
    /// rather than the geometry the plane removed.
    pub accept: Option<&'a dyn Fn(Vec3f) -> bool>,
    /// Ignore hits beyond this distance.
    pub max_t: f32,
    /// Skip triangles facing away from the ray.
    pub cull_backfaces: bool,
}

impl<'a> PickOptions<'a> {
    pub fn new(visible: &'a dyn Fn(ElementId) -> bool) -> Self {
        PickOptions {
            visible,
            accept: None,
            max_t: f32::INFINITY,
            cull_backfaces: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    prims: Vec<Prim>,
    /// One entry per batch: bounds of that batch (mirror of `RenderBatch::bounds`).
    batch_bounds: Vec<Aabb>,
    /// World bounds per element, sorted by element id. The culling index.
    element_bounds: Vec<(ElementId, Aabb)>,
    triangle_count: usize,
}

impl Bvh {
    pub fn build(batches: &[RenderBatch]) -> Bvh {
        Bvh::build_with(batches, &mut |_| {})
    }

    /// `progress` gets 0..=1 while the tree is built (5 M triangles is not
    /// instant; the loader shows a bar).
    pub fn build_with(batches: &[RenderBatch], progress: &mut dyn FnMut(f32)) -> Bvh {
        let triangle_count: usize = batches.iter().map(|b| b.triangle_count()).sum();
        let mut prims: Vec<Prim> = Vec::with_capacity(triangle_count);
        let mut bounds: Vec<Aabb> = Vec::with_capacity(triangle_count);
        let mut element_map: Vec<(ElementId, Aabb)> = Vec::new();

        for (bi, batch) in batches.iter().enumerate() {
            for r in &batch.element_ranges {
                let mut eb = aabb_empty();
                let first_tri = r.first_index / 3;
                let tris = r.index_count / 3;
                for t in first_tri..first_tri + tris {
                    let (a, b, c) = batch.triangle(t);
                    let tb = Aabb {
                        min: Vec3f::min_componentwise(Vec3f::min_componentwise(a, b), c),
                        max: Vec3f::max_componentwise(Vec3f::max_componentwise(a, b), c),
                    };
                    eb = aabb_union(&eb, &tb);
                    prims.push(Prim {
                        batch: bi as u32,
                        tri: t,
                        element: r.element.0,
                    });
                    bounds.push(tb);
                }
                element_map.push((r.element, eb));
            }
            progress(0.5 * (bi + 1) as f32 / batches.len().max(1) as f32);
        }

        // Merge the per-range element bounds into one entry per element.
        element_map.sort_by_key(|(e, _)| e.0);
        let mut element_bounds: Vec<(ElementId, Aabb)> = Vec::with_capacity(element_map.len());
        for (e, b) in element_map {
            match element_bounds.last_mut() {
                Some((le, lb)) if *le == e => *lb = aabb_union(lb, &b),
                _ => element_bounds.push((e, b)),
            }
        }

        let mut bvh = Bvh {
            nodes: Vec::new(),
            prims: Vec::new(),
            batch_bounds: batches.iter().map(|b| b.bounds).collect(),
            element_bounds,
            triangle_count,
        };
        if prims.is_empty() {
            progress(1.0);
            return bvh;
        }

        // order[] is the permutation the tree sorts; prims are rewritten into
        // leaf order at the end so traversal is linear in memory.
        let n = prims.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(2 * (n / MAX_LEAF).max(1) + 1);
        nodes.push(new_node(&bounds, &order[..]));
        let mut stack: Vec<(usize, usize, usize)> = vec![(0, 0, n)];
        let mut done = 0usize;
        while let Some((node_index, start, end)) = stack.pop() {
            let count = end - start;
            if count <= MAX_LEAF {
                nodes[node_index].first = start as u32;
                nodes[node_index].count = count as u32;
                done += count;
                if done % 65536 < count {
                    progress(0.5 + 0.5 * done as f32 / n as f32);
                }
                continue;
            }
            let split = match sah_split(&bounds, &mut order[start..end]) {
                Some(s) => start + s,
                None => {
                    nodes[node_index].first = start as u32;
                    nodes[node_index].count = count as u32;
                    done += count;
                    continue;
                }
            };
            let left = nodes.len();
            nodes.push(new_node(&bounds, &order[start..split]));
            let right = nodes.len();
            nodes.push(new_node(&bounds, &order[split..end]));
            nodes[node_index].first = left as u32;
            nodes[node_index].count = 0;
            stack.push((right, split, end));
            stack.push((left, start, split));
        }

        let mut sorted: Vec<Prim> = Vec::with_capacity(n);
        for &i in &order {
            sorted.push(prims[i as usize]);
        }
        bvh.nodes = nodes;
        bvh.prims = sorted;
        progress(1.0);
        bvh
    }

    pub fn triangle_count(&self) -> usize {
        self.triangle_count
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// World bounds of one element (empty when it has no geometry).
    pub fn element_bounds(&self, id: ElementId) -> Option<Aabb> {
        self.element_bounds
            .binary_search_by_key(&id.0, |(e, _)| e.0)
            .ok()
            .map(|i| self.element_bounds[i].1)
    }

    /// Every element the index knows about, with its world bounds.
    pub fn elements(&self) -> &[(ElementId, Aabb)] {
        &self.element_bounds
    }

    /// Closest hit along `ray`, skipping elements for which `visible` is false.
    /// `batches` must be the same slice the index was built from.
    pub fn raycast(
        &self,
        batches: &[RenderBatch],
        ray: &Ray,
        visible: &dyn Fn(ElementId) -> bool,
    ) -> Option<RayHit> {
        self.raycast_opt(batches, ray, &PickOptions::new(visible))
    }

    /// Closest hit with the full filter set (visibility, section half-spaces,
    /// distance clamp, back-face culling).
    pub fn raycast_opt(
        &self,
        batches: &[RenderBatch],
        ray: &Ray,
        opt: &PickOptions,
    ) -> Option<RayHit> {
        self.raycast_opt_filtered(batches, ray, opt, None)
    }

    /// Closest hit accepted by both the element predicate and `accept_hit`.
    ///
    /// Unlike restarting a ray after each rejected surface, this walks the
    /// BVH once. Navigation uses it to ignore open door leaves during leaf
    /// traversal and to choose the nearest upward floor above a site skirt.
    pub fn raycast_filtered(
        &self,
        batches: &[RenderBatch],
        ray: &Ray,
        visible: &dyn Fn(ElementId) -> bool,
        accept_hit: &dyn Fn(&RayHit) -> bool,
    ) -> Option<RayHit> {
        self.raycast_opt_filtered(batches, ray, &PickOptions::new(visible), Some(accept_hit))
    }

    fn raycast_opt_filtered(
        &self,
        batches: &[RenderBatch],
        ray: &Ray,
        opt: &PickOptions,
        accept_hit: Option<&dyn Fn(&RayHit) -> bool>,
    ) -> Option<RayHit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = inv_dir(ray);
        let mut best: Option<RayHit> = None;
        let mut best_t = opt.max_t;
        // (node, entry distance) — nearest child first, skip when already worse.
        let mut stack: Vec<(u32, f32)> = Vec::with_capacity(48);
        stack.push((0, 0.0));
        while let Some((ni, t_entry)) = stack.pop() {
            if t_entry >= best_t {
                continue;
            }
            let node = self.nodes[ni as usize];
            if node.count > 0 {
                let from = node.first as usize;
                let to = from + node.count as usize;
                for p in &self.prims[from..to] {
                    if !(opt.visible)(ElementId(p.element)) {
                        continue;
                    }
                    let batch = &batches[p.batch as usize];
                    let (pa, pb, pc) = batch.triangle(p.tri);
                    let Some((t, u, v)) = ray.intersect_triangle(pa, pb, pc) else {
                        continue;
                    };
                    if t >= best_t {
                        continue;
                    }
                    let mut n = Vec3f::cross(pb - pa, pc - pa).normalize();
                    let facing = n.dot(ray.dir) <= 0.0;
                    if opt.cull_backfaces && !facing {
                        continue;
                    }
                    let point = ray.at(t);
                    if let Some(accept) = opt.accept {
                        if !accept(point) {
                            continue;
                        }
                    }
                    if !facing {
                        n = -n;
                    }
                    let hit = RayHit {
                        element: ElementId(p.element),
                        batch: p.batch,
                        triangle: p.tri,
                        t,
                        point,
                        normal: n,
                        bary: [u, v],
                    };
                    if accept_hit.is_some_and(|accept| !accept(&hit)) {
                        continue;
                    }
                    best_t = t;
                    best = Some(hit);
                }
                continue;
            }
            let left = node.first as usize;
            let right = left + 1;
            let tl = slab(&self.nodes[left], ray, &inv);
            let tr = slab(&self.nodes[right], ray, &inv);
            match (tl, tr) {
                (Some(a), Some(b)) => {
                    if a <= b {
                        stack.push((right as u32, b));
                        stack.push((left as u32, a));
                    } else {
                        stack.push((left as u32, a));
                        stack.push((right as u32, b));
                    }
                }
                (Some(a), None) => stack.push((left as u32, a)),
                (None, Some(b)) => stack.push((right as u32, b)),
                (None, None) => {}
            }
        }
        best
    }

    /// Every triangle whose bounds touch the sphere, as
    /// `(batch, triangle, element)`. Used by measurement snapping to gather the
    /// vertices/edges near the cursor.
    pub fn triangles_in_sphere(
        &self,
        center: Vec3f,
        radius: f32,
        visible: &dyn Fn(ElementId) -> bool,
        out: &mut Vec<(u32, u32, ElementId)>,
    ) {
        out.clear();
        if self.nodes.is_empty() || radius <= 0.0 {
            return;
        }
        let r2 = radius * radius;
        let mut stack: Vec<u32> = vec![0];
        while let Some(ni) = stack.pop() {
            let node = self.nodes[ni as usize];
            if sphere_box_dist2(center, &node.bounds()) > r2 {
                continue;
            }
            if node.count > 0 {
                let from = node.first as usize;
                let to = from + node.count as usize;
                for p in &self.prims[from..to] {
                    if (visible)(ElementId(p.element)) {
                        out.push((p.batch, p.tri, ElementId(p.element)));
                    }
                }
            } else {
                stack.push(node.first);
                stack.push(node.first + 1);
            }
        }
    }

    /// Every element whose world bounds intersect the frustum, sorted by id.
    pub fn frustum_elements(
        &self,
        _batches: &[RenderBatch],
        frustum: &Frustum,
        out: &mut Vec<ElementId>,
    ) {
        out.clear();
        for (e, b) in &self.element_bounds {
            if frustum.intersects_aabb(b) {
                out.push(*e);
            }
        }
    }

    /// Bounds of every batch, in batch order.
    pub fn batch_bounds(&self) -> &[Aabb] {
        &self.batch_bounds
    }

    /// Bounds of one contiguous index range inside one batch.
    pub fn range_bounds(batch: &RenderBatch, first_index: u32, index_count: u32) -> Aabb {
        let mut b = aabb_empty();
        let end = (first_index + index_count) as usize;
        for i in first_index as usize..end {
            b = aabb_union_point(&b, batch.position(batch.indices[i]));
        }
        b
    }
}

fn new_node(bounds: &[Aabb], order: &[u32]) -> Node {
    let mut b = aabb_empty();
    for &i in order {
        b = aabb_union(&b, &bounds[i as usize]);
    }
    if aabb_is_empty(&b) {
        b = Aabb {
            min: Vec3f::default(),
            max: Vec3f::default(),
        };
    }
    Node {
        min: [b.min.x, b.min.y, b.min.z],
        max: [b.max.x, b.max.y, b.max.z],
        first: 0,
        count: 0,
    }
}

fn axis(v: Vec3f, a: usize) -> f32 {
    match a {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn surface(b: &Aabb) -> f32 {
    let e = b.max - b.min;
    if e.x < 0.0 {
        return 0.0;
    }
    2.0 * (e.x * e.y + e.y * e.z + e.z * e.x)
}

/// Binned SAH split of `order` in place. Returns the pivot, or `None` when a
/// leaf is cheaper (or the centroids are degenerate).
fn sah_split(bounds: &[Aabb], order: &mut [u32]) -> Option<usize> {
    let n = order.len();
    let mut cb = aabb_empty();
    for &i in order.iter() {
        let c = (bounds[i as usize].min + bounds[i as usize].max) * 0.5;
        cb = aabb_union_point(&cb, c);
    }
    let ext = cb.max - cb.min;
    let a = if ext.x > ext.y && ext.x > ext.z {
        0
    } else if ext.y > ext.z {
        1
    } else {
        2
    };
    let width = axis(ext, a);
    if !(width > 1e-9) {
        // All centroids coincide: split down the middle so the tree still
        // terminates (huge coincident fans do happen in CAD exports).
        return Some(n / 2);
    }
    let lo = axis(cb.min, a);
    let scale = BINS as f32 / width;

    let mut bin_count = [0u32; BINS];
    let mut bin_bounds = [aabb_empty(); BINS];
    let bin_of = |i: u32| -> usize {
        let c = (axis(bounds[i as usize].min, a) + axis(bounds[i as usize].max, a)) * 0.5;
        (((c - lo) * scale) as usize).min(BINS - 1)
    };
    for &i in order.iter() {
        let b = bin_of(i);
        bin_count[b] += 1;
        bin_bounds[b] = aabb_union(&bin_bounds[b], &bounds[i as usize]);
    }

    // Prefix/suffix sweeps.
    let mut left_area = [0f32; BINS];
    let mut left_n = [0u32; BINS];
    let mut acc = aabb_empty();
    let mut cnt = 0u32;
    for b in 0..BINS {
        acc = aabb_union(&acc, &bin_bounds[b]);
        cnt += bin_count[b];
        left_area[b] = surface(&acc);
        left_n[b] = cnt;
    }
    let mut best_cost = f32::INFINITY;
    let mut best_bin = usize::MAX;
    let mut acc = aabb_empty();
    let mut cnt = 0u32;
    for b in (1..BINS).rev() {
        acc = aabb_union(&acc, &bin_bounds[b]);
        cnt += bin_count[b];
        let ln = left_n[b - 1];
        if ln == 0 || cnt == 0 {
            continue;
        }
        let cost = TRAV_COST + left_area[b - 1] * ln as f32 + surface(&acc) * cnt as f32;
        if cost < best_cost {
            best_cost = cost;
            best_bin = b;
        }
    }
    if best_bin == usize::MAX {
        return Some(n / 2);
    }

    // Partition in place.
    let mut i = 0usize;
    let mut j = n;
    while i < j {
        if bin_of(order[i]) < best_bin {
            i += 1;
        } else {
            j -= 1;
            order.swap(i, j);
        }
    }
    if i == 0 || i == n {
        Some(n / 2)
    } else {
        Some(i)
    }
}

fn inv_dir(ray: &Ray) -> [f32; 3] {
    [
        1.0 / if ray.dir.x == 0.0 { 1e-20 } else { ray.dir.x },
        1.0 / if ray.dir.y == 0.0 { 1e-20 } else { ray.dir.y },
        1.0 / if ray.dir.z == 0.0 { 1e-20 } else { ray.dir.z },
    ]
}

/// Slab test against a node; returns the entry distance (0 when inside).
fn slab(node: &Node, ray: &Ray, inv: &[f32; 3]) -> Option<f32> {
    let o = [ray.origin.x, ray.origin.y, ray.origin.z];
    let mut tmin = 0.0f32;
    let mut tmax = f32::INFINITY;
    for i in 0..3 {
        let t0 = (node.min[i] - o[i]) * inv[i];
        let t1 = (node.max[i] - o[i]) * inv[i];
        let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
        tmin = tmin.max(t0);
        tmax = tmax.min(t1);
        if tmin > tmax {
            return None;
        }
    }
    Some(tmin)
}

fn sphere_box_dist2(p: Vec3f, b: &Aabb) -> f32 {
    let dx = (b.min.x - p.x).max(0.0).max(p.x - b.max.x);
    let dy = (b.min.y - p.y).max(0.0).max(p.y - b.max.y);
    let dz = (b.min.z - p.z).max(0.0).max(p.z - b.max.z);
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::batch::{ElementRange, Vertex};
    use crate::model::ids::MaterialId;

    /// A batch of `n` unit quads scattered on a grid, one element each.
    fn grid_batch(n: usize) -> RenderBatch {
        let mut b = RenderBatch {
            material: MaterialId::from_index(0),
            ..Default::default()
        };
        let side = (n as f32).sqrt().ceil() as usize;
        for i in 0..n {
            let (gx, gy) = ((i % side) as f32 * 2.0, (i / side) as f32 * 2.0);
            let first = b.indices.len() as u32;
            let base = b.vertex_count() as u32;
            for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                b.push_vertex(&Vertex {
                    position: [gx + dx, gy + dy, (i % 7) as f32 * 0.25],
                    element: i as f32,
                    normal: [0.0, 0.0, 1.0],
                    uv: [dx, dy],
                    ..Default::default()
                });
            }
            b.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            b.element_ranges.push(ElementRange {
                element: ElementId::from_index(i),
                part: i as u32,
                first_index: first,
                index_count: 6,
                draw_priority: 0,
                coplanar_group: 0,
            });
        }
        let mut bounds = aabb_empty();
        for v in 0..b.vertex_count() as u32 {
            bounds = aabb_union_point(&bounds, b.position(v));
        }
        b.bounds = bounds;
        b
    }

    fn stacked_batch() -> RenderBatch {
        let mut batch = RenderBatch {
            material: MaterialId::from_index(0),
            ..Default::default()
        };
        for (element, z) in [(0usize, 1.0f32), (1, 0.0)] {
            let first = batch.indices.len() as u32;
            let base = batch.vertex_count() as u32;
            for position in [[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]] {
                batch.push_vertex(&Vertex {
                    position,
                    element: element as f32,
                    normal: [0.0, 0.0, 1.0],
                    ..Default::default()
                });
            }
            batch.indices.extend_from_slice(&[base, base + 1, base + 2]);
            batch.element_ranges.push(ElementRange {
                element: ElementId::from_index(element),
                part: 0,
                first_index: first,
                index_count: 3,
                draw_priority: 0,
                coplanar_group: 0,
            });
        }
        batch.bounds = Aabb {
            min: Vec3f::default(),
            max: Vec3f {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        };
        batch
    }

    fn brute(batches: &[RenderBatch], ray: &Ray) -> Option<(ElementId, f32)> {
        let mut best: Option<(ElementId, f32)> = None;
        for batch in batches {
            for t in 0..batch.triangle_count() as u32 {
                let (a, b, c) = batch.triangle(t);
                if let Some((h, _, _)) = ray.intersect_triangle(a, b, c) {
                    if best.map_or(true, |(_, bt)| h < bt) {
                        best = Some((batch.element_of_triangle(t).unwrap(), h));
                    }
                }
            }
        }
        best
    }

    #[test]
    fn bvh_agrees_with_brute_force() {
        let batches = vec![grid_batch(400)];
        let bvh = Bvh::build(&batches);
        assert_eq!(bvh.triangle_count(), 800);
        let all = |_: ElementId| true;
        let mut seed = 12345u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 16777216.0
        };
        let mut hits = 0;
        for _ in 0..500 {
            let origin = Vec3f {
                x: rnd() * 42.0 - 1.0,
                y: rnd() * 42.0 - 1.0,
                z: 10.0,
            };
            let dir = Vec3f {
                x: rnd() * 0.4 - 0.2,
                y: rnd() * 0.4 - 0.2,
                z: -1.0,
            };
            let ray = Ray::new(origin, dir);
            let a = bvh.raycast(&batches, &ray, &all).map(|h| (h.element, h.t));
            let b = brute(&batches, &ray);
            match (a, b) {
                (Some((ea, ta)), Some((eb, tb))) => {
                    assert!((ta - tb).abs() < 1e-3, "t {ta} vs {tb}");
                    assert_eq!(ea, eb);
                    hits += 1;
                }
                (None, None) => {}
                (x, y) => panic!("mismatch {x:?} vs {y:?}"),
            }
        }
        assert!(hits > 100, "test rays never hit anything ({hits})");
    }

    #[test]
    fn invisible_elements_are_skipped() {
        let batches = vec![grid_batch(16)];
        let bvh = Bvh::build(&batches);
        let ray = Ray::new(
            Vec3f {
                x: 0.5,
                y: 0.5,
                z: 5.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let all = |_: ElementId| true;
        let hit = bvh.raycast(&batches, &ray, &all).unwrap();
        assert_eq!(hit.element, ElementId(0));
        let none = |e: ElementId| e != ElementId(0);
        assert!(bvh.raycast(&batches, &ray, &none).is_none());
    }

    #[test]
    fn accept_filter_cuts_the_front_half() {
        let batches = vec![grid_batch(4)];
        let bvh = Bvh::build(&batches);
        let ray = Ray::new(
            Vec3f {
                x: 0.5,
                y: 0.5,
                z: 5.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let all = |_: ElementId| true;
        let below = |p: Vec3f| p.z < -1.0;
        let opt = PickOptions {
            visible: &all,
            accept: Some(&below),
            max_t: f32::INFINITY,
            cull_backfaces: false,
        };
        assert!(bvh.raycast_opt(&batches, &ray, &opt).is_none());
    }

    #[test]
    fn hit_filter_finds_the_next_surface_in_one_traversal() {
        let batches = vec![stacked_batch()];
        let bvh = Bvh::build(&batches);
        let ray = Ray::new(
            Vec3f {
                x: 0.2,
                y: 0.2,
                z: 3.0,
            },
            Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
        );
        let all = |_: ElementId| true;
        assert_eq!(
            bvh.raycast(&batches, &ray, &all).unwrap().element,
            ElementId(0)
        );
        let lower = |hit: &RayHit| hit.point.z < 0.5;
        let hit = bvh
            .raycast_filtered(&batches, &ray, &all, &lower)
            .expect("lower accepted surface");
        assert_eq!(hit.element, ElementId(1));
        assert!((hit.point.z - 0.0).abs() < 1e-5);
    }

    #[test]
    fn element_bounds_and_frustum_scan() {
        let batches = vec![grid_batch(9)];
        let bvh = Bvh::build(&batches);
        assert_eq!(bvh.elements().len(), 9);
        let b = bvh.element_bounds(ElementId(0)).unwrap();
        assert!(b.min.x <= 0.0 && b.max.x >= 1.0);
        assert!(bvh.element_bounds(ElementId(99)).is_none());
    }

    #[test]
    fn empty_scene_is_safe() {
        let bvh = Bvh::build(&[]);
        let ray = Ray::new(Vec3f::default(), Vec3f { x: 0.0, y: 0.0, z: 1.0 });
        let all = |_: ElementId| true;
        assert!(bvh.raycast(&[], &ray, &all).is_none());
        let mut out = Vec::new();
        bvh.triangles_in_sphere(Vec3f::default(), 1.0, &all, &mut out);
        assert!(out.is_empty());
    }
}
