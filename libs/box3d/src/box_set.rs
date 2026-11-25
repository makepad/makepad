//! An editable set of solid, axis-aligned boxes in a common local frame.

use std::sync::Arc;

use crate::aabb::farthest_point_on_aabb;
use crate::b3_validate;
use crate::constants::{linear_slop, overlap_slop};
use crate::core::NULL_INDEX;
use crate::dynamic_tree::{
    dynamic_tree_create, dynamic_tree_create_proxy, dynamic_tree_destroy_proxy, dynamic_tree_get_height,
    dynamic_tree_get_root_bounds, dynamic_tree_query, dynamic_tree_query_closest, TREE_STACK_SIZE,
};
use crate::id_pool::{alloc_id, create_id_pool, free_id, IdPool};
use crate::math_functions::*;
use crate::math_internal::ShapeExtent;
use crate::recording::RecBuffer;
use crate::recording_replay::RecCursor;
use crate::types::{
    Capsule, CastOutput, DistanceInput, DynamicTree, MassData, PlaneResult, RayCastInput, ShapeCastInput,
    ShapeCastPairInput, ShapeProxy, SimplexCache, DEFAULT_MASK_BITS,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetBox {
    pub bounds: AABB,
    /// Index in the shape's material list.
    pub material: u32,
    pub user_data: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxSetError {
    InvalidBounds { child: i32 },
    OverlappingInteriors { child_a: i32, child_b: i32 },
}

#[derive(Clone, Debug)]
struct Slot {
    value: Option<SetBox>,
    proxy: i32,
    serial: u32,
}

/// Unit-density second moments about the current centre of mass, in f64.
#[derive(Clone, Debug, Default)]
struct Moments {
    volume: f64,
    center: [f64; 3],
    second: [f64; 3],
    /// Integrals of xy, yz and zx.
    products: [f64; 3],
}

fn dimensions(bounds: AABB) -> [f64; 3] {
    let lo = bounds.lower_bound;
    let hi = bounds.upper_bound;
    [hi.x as f64 - lo.x as f64, hi.y as f64 - lo.y as f64, hi.z as f64 - lo.z as f64]
}

fn valid_bounds(bounds: AABB) -> bool {
    let [x, y, z] = dimensions(bounds);
    let volume = (x * y * z) as f32;
    x > 0.0 && y > 0.0 && z > 0.0 && volume > 0.0 && volume.is_finite()
}

impl Moments {
    // Merge (or subtract) a box using the parallel-axis theorem in the
    // combined centre's frame. A subtraction that loses f32 precision
    // returns false so the caller can re-sum the surviving boxes.
    fn accumulate(&mut self, bounds: AABB, sign: f64) -> bool {
        let size = dimensions(bounds);
        let lo = bounds.lower_bound;
        let center = [lo.x as f64 + 0.5 * size[0], lo.y as f64 + 0.5 * size[1], lo.z as f64 + 0.5 * size[2]];
        let volume = sign * size[0] * size[1] * size[2];
        let total = self.volume + volume;
        const RELATIVE_ROUNDOFF: f64 = f64::EPSILON / f32::EPSILON as f64;
        if total <= self.volume * RELATIVE_ROUNDOFF {
            return false;
        }
        let box_fraction = volume / total;
        let set_fraction = self.volume / total;
        let weight = self.volume * box_fraction;
        let delta = [center[0] - self.center[0], center[1] - self.center[1], center[2] - self.center[2]];
        for i in 0..3 {
            let j = (i + 1) % 3;
            let previous = self.second[i];
            self.second[i] += volume * size[i] * size[i] / 12.0 + weight * delta[i] * delta[i];
            if sign < 0.0 && self.second[i] <= previous * RELATIVE_ROUNDOFF {
                return false;
            }
            self.products[i] += weight * delta[i] * delta[j];
            // The larger positive contributor is the more accurate reference.
            self.center[i] = if volume > self.volume {
                center[i] - set_fraction * delta[i]
            } else {
                self.center[i] + box_fraction * delta[i]
            };
        }
        self.volume = total;
        true
    }
}

#[derive(Clone, Debug)]
pub struct BoxSetData {
    tree: DynamicTree,
    slots: Vec<Slot>,
    ids: IdPool,
    moments: Moments,
    pub(crate) revision: u64,
}

impl BoxSetData {
    pub(crate) fn write_snapshot(&self, buf: &mut RecBuffer) {
        crate::world_snapshot::ser_tree(buf, &self.tree);
        crate::world_snapshot::ser_id_pool(buf, &self.ids);
        buf.w_u64(self.revision);
        let m = &self.moments;
        buf.w_f64(m.volume);
        for value in m.center.into_iter().chain(m.second).chain(m.products) {
            buf.w_f64(value);
        }
        buf.w_i32(self.slots.len() as i32);
        for slot in &self.slots {
            buf.w_i32(slot.proxy);
            buf.w_u32(slot.serial);
            buf.w_bool(slot.value.is_some());
            if let Some(value) = slot.value {
                buf.w_aabb(value.bounds);
                buf.w_u32(value.material);
                buf.w_u64(value.user_data);
            }
        }
    }

    pub(crate) fn read_snapshot(r: &mut RecCursor) -> Option<Self> {
        let mut tree = DynamicTree::default();
        crate::world_snapshot::des_tree(r, &mut tree);
        let mut ids = IdPool::default();
        crate::world_snapshot::des_id_pool(r, &mut ids);
        let revision = r.r_u64();
        let moments = Moments {
            volume: r.r_f64(),
            center: std::array::from_fn(|_| r.r_f64()),
            second: std::array::from_fn(|_| r.r_f64()),
            products: std::array::from_fn(|_| r.r_f64()),
        };
        let count = r.r_i32();
        if !r.check_count(count, 9) || ids.next_index != count {
            return None;
        }
        let mut slots = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let proxy = r.r_i32();
            let serial = r.r_u32();
            let value = match r.r_u8() {
                0 => None,
                1 => Some(SetBox { bounds: r.r_aabb(), material: r.r_u32(), user_data: r.r_u64() }),
                _ => return None,
            };
            slots.push(Slot { value, proxy, serial });
        }
        let set = Self { tree, ids, slots, moments, revision };
        (r.ok && r.remaining() == 0 && set.snapshot_links_valid()).then_some(set)
    }

    // Check the two free lists and the tree-to-slot links before the first
    // query or edit can follow an index supplied by a registry blob.
    fn snapshot_links_valid(&self) -> bool {
        use crate::types::{ALLOCATED_NODE, LEAF_NODE};
        let tree = &self.tree;
        if tree.node_capacity < 2 || tree.node_capacity as usize != tree.nodes.len() {
            return false;
        }
        let mut free_slots = vec![false; self.slots.len()];
        for &index in &self.ids.free_array {
            let Some(free) = free_slots.get_mut(index as usize) else { return false };
            if std::mem::replace(free, true) {
                return false;
            }
        }
        let mut proxies = 0;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.serial == 0 || free_slots[index] != (slot.value.is_none() && slot.serial != u32::MAX) {
                return false;
            }
            // An importer can retain malformed values without indexing them.
            if slot.proxy == NULL_INDEX {
                if slot.value.is_some_and(|v| valid_bounds(v.bounds)) {
                    return false;
                }
            } else {
                let Some(node) = tree.nodes.get(slot.proxy as usize) else { return false };
                if node.flags & LEAF_NODE == 0
                    || node.user_data != index as u64
                    || !slot.value.is_some_and(|v| v.bounds == node.aabb && valid_bounds(v.bounds))
                {
                    return false;
                }
                proxies += 1;
            }
        }
        if proxies != tree.proxy_count || (proxies == 0) != (tree.root == NULL_INDEX) {
            return false;
        }
        let mut seen = vec![false; tree.nodes.len()];
        let mut pending = Vec::new();
        if tree.root != NULL_INDEX {
            pending.push((tree.root, NULL_INDEX));
        }
        let mut live = 0;
        while let Some((id, parent)) = pending.pop() {
            let Some(node) = tree.nodes.get(id as usize) else { return false };
            if std::mem::replace(&mut seen[id as usize], true) || node.parent != parent || node.flags & ALLOCATED_NODE == 0 {
                return false;
            }
            live += 1;
            if node.flags & LEAF_NODE != 0 {
                let slot = usize::try_from(node.user_data).ok().and_then(|i| self.slots.get(i));
                if node.height != 0 || slot.map(|s| s.proxy) != Some(id) {
                    return false;
                }
            } else {
                let children = [node.children.child1, node.children.child2];
                for child in children {
                    if !tree.nodes.get(child as usize).is_some_and(|n| n.height < node.height) {
                        return false;
                    }
                    pending.push((child, id));
                }
            }
        }
        if live != tree.node_count {
            return false;
        }
        let mut free = tree.free_list;
        while free != NULL_INDEX {
            let Some(node) = tree.nodes.get(free as usize) else { return false };
            if std::mem::replace(&mut seen[free as usize], true) || node.flags & ALLOCATED_NODE != 0 {
                return false;
            }
            free = node.parent;
        }
        seen.into_iter().all(|visited| visited)
    }

    /// Enumerate live children in ascending slot order, including after edits.
    pub fn boxes(&self) -> impl Iterator<Item = (i32, &SetBox)> {
        self.slots.iter().enumerate().filter_map(|(i, slot)| Some((i as i32, slot.value.as_ref()?)))
    }

    /// Local bounds. An empty set has no broad-phase proxy.
    pub fn bounds(&self) -> Option<AABB> {
        (self.tree.root != NULL_INDEX).then(|| dynamic_tree_get_root_bounds(&self.tree))
    }

    /// The tree over the indexed boxes: leaf `user_data` is the child slot,
    /// and a leaf's bounds are its box's bounds.
    pub(crate) fn tree(&self) -> &DynamicTree {
        &self.tree
    }

    pub(crate) fn indexed_box_count(&self) -> i32 {
        crate::dynamic_tree::dynamic_tree_get_proxy_count(&self.tree)
    }

    pub(crate) fn serial(&self, child: i32) -> Option<u32> {
        let slot = self.slots.get(child as usize)?;
        slot.value.as_ref().map(|_| slot.serial)
    }

    fn insert(&mut self, value: SetBox) -> i32 {
        let child = alloc_id(&mut self.ids);
        // Keep malformed input addressable so an importer can obtain its
        // slot from box_set_validate without putting a NaN in the tree.
        let proxy = if valid_bounds(value.bounds) {
            self.moments.accumulate(value.bounds, 1.0);
            dynamic_tree_create_proxy(&mut self.tree, value.bounds, DEFAULT_MASK_BITS, child as u64)
        } else {
            NULL_INDEX
        };
        if child as usize == self.slots.len() {
            self.slots.push(Slot { value: Some(value), proxy, serial: 1 });
        } else {
            let slot = &mut self.slots[child as usize];
            slot.value = Some(value);
            slot.proxy = proxy;
            slot.serial += 1;
        }
        child
    }

    fn remove(&mut self, child: i32) -> bool {
        let Some(slot) = self.slots.get_mut(child as usize) else { return false };
        let Some(value) = slot.value.take() else { return false };
        let proxy = std::mem::replace(&mut slot.proxy, NULL_INDEX);
        // Retire a slot when its next insertion would alias its first lifetime.
        if slot.serial < u32::MAX {
            free_id(&mut self.ids, child);
        } else {
            eprintln!("box3d: retiring box set slot {child} after serial exhaustion");
        }
        if proxy != NULL_INDEX {
            dynamic_tree_destroy_proxy(&mut self.tree, proxy);
            if self.tree.root == NULL_INDEX {
                self.moments = Moments::default();
            } else if !self.moments.accumulate(value.bounds, -1.0) {
                let mut moments = Moments::default();
                for (_, value) in self.boxes() {
                    if valid_bounds(value.bounds) {
                        moments.accumulate(value.bounds, 1.0);
                    }
                }
                self.moments = moments;
            }
        }
        true
    }

    pub(crate) fn edit(&mut self, remove: &[i32], add: &[SetBox], added: &mut Vec<i32>) {
        added.clear();
        let mut changed = false;
        for &child in remove {
            changed |= self.remove(child);
        }
        for &value in add {
            b3_validate!(valid_bounds(value.bounds));
            b3_validate!(self.overlapping_interior(value.bounds, NULL_INDEX).is_none());
            added.push(self.insert(value));
        }
        if changed || !add.is_empty() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    // A traversal can have at most height + 1 pending nodes. Use a linear
    // scan if the existing tree traversal's stack cannot hold that many.
    fn fits_tree_stack(&self) -> bool {
        dynamic_tree_get_height(&self.tree) < TREE_STACK_SIZE as i32
    }

    pub(crate) fn query(&self, bounds: AABB, visit: &mut impl FnMut(i32, &SetBox) -> bool) {
        if self.fits_tree_stack() {
            dynamic_tree_query(&self.tree, bounds, DEFAULT_MASK_BITS, false, &mut |_, data| {
                let child = data as i32;
                visit(child, self.slots[child as usize].value.as_ref().unwrap())
            });
        } else {
            for (child, value) in self.boxes() {
                if valid_bounds(value.bounds) && aabb_overlaps(bounds, value.bounds) && !visit(child, value) {
                    break;
                }
            }
        }
    }

    // The tree includes face, edge and corner touches. Only a strict
    // intersection on all three axes violates the disjoint-interior rule.
    fn overlapping_interior(&self, bounds: AABB, skip_through: i32) -> Option<i32> {
        let mut overlap = None;
        self.query(bounds, &mut |child, value| {
            if child > skip_through {
                let lo = max(bounds.lower_bound, value.bounds.lower_bound);
                let hi = min(bounds.upper_bound, value.bounds.upper_bound);
                if lo.x < hi.x && lo.y < hi.y && lo.z < hi.z {
                    overlap = Some(child);
                    return false;
                }
            }
            true
        });
        overlap
    }
}

/// Build in input order. Importers can inspect errors with box_set_validate
/// before attaching the set to a shape.
pub fn create_box_set(boxes: &[SetBox]) -> Arc<BoxSetData> {
    let mut set = BoxSetData {
        tree: dynamic_tree_create(boxes.len() as i32),
        slots: Vec::with_capacity(boxes.len()),
        ids: create_id_pool(),
        moments: Moments::default(),
        revision: 0,
    };
    for &value in boxes {
        set.insert(value);
    }
    Arc::new(set)
}

pub fn box_set_validate(set: &BoxSetData) -> Result<(), BoxSetError> {
    for (child, value) in set.boxes() {
        if !valid_bounds(value.bounds) {
            return Err(BoxSetError::InvalidBounds { child });
        }
        if let Some(other) = set.overlapping_interior(value.bounds, child) {
            return Err(BoxSetError::OverlappingInteriors { child_a: child, child_b: other });
        }
    }
    Ok(())
}

pub fn box_set_get(set: &BoxSetData, child: i32) -> Option<&SetBox> {
    set.slots.get(child as usize)?.value.as_ref()
}

/// Visit boxes intersecting the closed bounds in the set's local coordinates.
/// Return false to stop. Traversal order is unspecified.
pub fn box_set_query(set: &BoxSetData, bounds: AABB, mut visit: impl FnMut(i32, &SetBox) -> bool) {
    set.query(bounds, &mut visit);
}

pub fn compute_box_set_aabb(set: &BoxSetData, transform: Transform) -> Option<AABB> {
    set.bounds().map(|bounds| transform_box_bounds(bounds, transform))
}

pub(crate) fn transform_box_bounds(bounds: AABB, transform: Transform) -> AABB {
    transform_box_bounds_by(bounds, make_matrix_from_quat(transform.q), transform.p)
}

/// [`transform_box_bounds`] with the rotation already built, for callers that
/// transform many boxes by one pose. Both produce the same bits.
pub(crate) fn transform_box_bounds_by(bounds: AABB, rotation: Matrix3, translation: Vec3) -> AABB {
    // Project interval endpoints directly. A thin box's rounded midpoint
    // can coincide with an endpoint even under the identity transform.
    let lo = bounds.lower_bound;
    let hi = bounds.upper_bound;
    let mut lower = translation;
    let mut upper = translation;
    for (axis, start, end) in [(rotation.cx, lo.x, hi.x), (rotation.cy, lo.y, hi.y), (rotation.cz, lo.z, hi.z)] {
        let a = mul_sv(start, axis);
        let b = mul_sv(end, axis);
        lower = add(lower, min(a, b));
        upper = add(upper, max(a, b));
    }
    AABB { lower_bound: lower, upper_bound: upper }
}

pub fn compute_box_set_extent(set: &BoxSetData, local_center: Vec3) -> ShapeExtent {
    let Some(bounds) = set.bounds() else { return ShapeExtent::default() };
    let half = aabb_extents(bounds);
    let corner = farthest_point_on_aabb(bounds, local_center);
    ShapeExtent { min_extent: half.x.min(half.y).min(half.z), max_extent: abs(sub(corner, local_center)) }
}

pub fn compute_box_set_mass(set: &BoxSetData, density: f32) -> MassData {
    let m = &set.moments;
    if set.tree.root == NULL_INDEX {
        return MassData::default();
    }
    let [xx, yy, zz] = m.second.map(|v| v * density as f64);
    let [xy, yz, zx] = m.products.map(|v| (-v * density as f64) as f32);
    MassData {
        mass: (density as f64 * m.volume) as f32,
        center: vec3(m.center[0] as f32, m.center[1] as f32, m.center[2] as f32),
        inertia: Matrix3 {
            cx: vec3((yy + zz) as f32, xy, zx),
            cy: vec3(xy, (zz + xx) as f32, yz),
            cz: vec3(zx, yz, (xx + yy) as f32),
        },
    }
}

// Intersect the three slabs in ray-fraction coordinates. A point initially
// inside (including a zero-length ray) has fraction zero and no entry face.
fn ray_entry(bounds: AABB, input: &RayCastInput) -> Option<(f32, Vec3)> {
    let lo = bounds.lower_bound;
    let hi = bounds.upper_bound;
    let p = input.origin;
    let d = input.translation;
    let mut lower = 0.0;
    let mut upper = input.max_fraction;
    let mut normal = Vec3::ZERO;
    for (start, delta, min, max, axis) in [
        (p.x, d.x, lo.x, hi.x, Vec3::AXIS_X),
        (p.y, d.y, lo.y, hi.y, Vec3::AXIS_Y),
        (p.z, d.z, lo.z, hi.z, Vec3::AXIS_Z),
    ] {
        if delta == 0.0 {
            if start < min || max < start {
                return None;
            }
            continue;
        }
        let (near, far, face) = if delta > 0.0 { (min, max, neg(axis)) } else { (max, min, axis) };
        let entry = (near - start) / delta;
        if lower < entry {
            lower = entry;
            normal = face;
        }
        upper = upper.min((far - start) / delta);
        if upper < lower {
            return None;
        }
    }
    Some((lower, normal))
}

/// First entry into the union. Shared-face and shared-edge ties go to the
/// lowest slot, independent of tree topology and edit history.
pub fn ray_cast_box_set(set: &BoxSetData, input: &RayCastInput) -> CastOutput {
    b3_validate!(is_valid_ray(input));
    // Query the endpoints' bounds, then clip against the exact slabs.
    // The tree's ray SAT reconstructs a half-width from a rounded centre,
    // which can cull a ray on the face of a thin box.
    let end = mul_add(input.origin, input.max_fraction, input.translation);
    let bounds = AABB { lower_bound: min(input.origin, end), upper_bound: max(input.origin, end) };
    let mut result = CastOutput::default();
    let mut ray = *input;
    set.query(bounds, &mut |child, value| {
        let Some((fraction, normal)) = ray_entry(value.bounds, &ray) else { return true };
        if !result.hit || fraction < result.fraction || (fraction == result.fraction && child < result.child_index) {
            ray.max_fraction = fraction;
            result = CastOutput {
                hit: true,
                fraction,
                normal,
                point: mul_add(ray.origin, fraction, ray.translation),
                child_index: child,
                triangle_index: NULL_INDEX,
                // Shape material lists use signed indices. The child still
                // carries the full importer value when it cannot index a list.
                material_index: i32::try_from(value.material).unwrap_or(NULL_INDEX),
                ..Default::default()
            };
        }
        true
    });
    result
}

#[inline]
pub(crate) fn box_vertices(bounds: AABB) -> [Vec3; 8] {
    let lo = bounds.lower_bound;
    let hi = bounds.upper_bound;
    [hi, vec3(lo.x, hi.y, hi.z), vec3(lo.x, lo.y, hi.z), vec3(hi.x, lo.y, hi.z),
        vec3(hi.x, hi.y, lo.z), vec3(lo.x, hi.y, lo.z), lo, vec3(hi.x, lo.y, lo.z)]
}

pub(crate) fn overlap_box_set(set: &BoxSetData, proxy: &ShapeProxy, proxy_to_set: Transform) -> bool {
    let bounds = transform_box_bounds(make_aabb(proxy.points, proxy.radius), proxy_to_set);
    let mut found = false;
    set.query(aabb_inflate(bounds, overlap_slop()), &mut |_, child| {
        let vertices = box_vertices(child.bounds);
        let input = DistanceInput {
            proxy_a: ShapeProxy { points: &vertices, radius: 0.0 },
            proxy_b: *proxy,
            transform: proxy_to_set,
            use_radii: true,
        };
        found = crate::distance::shape_distance(&input, &mut SimplexCache::default(), None).distance < overlap_slop();
        !found
    });
    found
}

pub(crate) fn closest_box_set_point(set: &BoxSetData, target: Vec3) -> Option<Vec3> {
    set.bounds()?;
    // Compare in f64 so squaring a large, finite local distance does not overflow.
    let distance_to = |point: Vec3| {
        let dx = point.x as f64 - target.x as f64;
        let dy = point.y as f64 - target.y as f64;
        let dz = point.z as f64 - target.z as f64;
        dx * dx + dy * dy + dz * dz
    };
    let mut best = (f64::INFINITY, i32::MAX, target);
    let consider = |bounds: AABB, slot: i32, best: &mut (f64, i32, Vec3)| {
        let point = clamp(target, bounds.lower_bound, bounds.upper_bound);
        let distance = distance_to(point);
        if (distance, slot) < (best.0, best.1) {
            *best = (distance, slot, point);
        }
    };
    let mut limit = f32::INFINITY;
    if set.fits_tree_stack() {
        dynamic_tree_query_closest(&set.tree, target, DEFAULT_MASK_BITS, false, &mut |_, node, slot| {
            consider(set.tree.nodes[node as usize].aabb, slot as i32, &mut best);
            // The tree uses a strict f32 comparison. Round outwards through
            // the subtraction and dot product to include equal-distance leaves.
            ((best.0 as f32) * (1.0 + 8.0 * f32::EPSILON)).max(f32::MIN_POSITIVE).next_up()
        }, &mut limit);
    }
    if !limit.is_finite() {
        // Also covers a root whose squared distance overflows the tree's f32.
        for (slot, child) in set.boxes().filter(|(_, b)| valid_bounds(b.bounds)) {
            consider(child.bounds, slot, &mut best);
        }
    }
    Some(best.2)
}

const AXES: [Vec3; 3] = [Vec3::AXIS_X, Vec3::AXIS_Y, Vec3::AXIS_Z];

fn seam_tolerance() -> f32 {
    0.1 * linear_slop()
}

fn coords(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

// At flush faces the witness and its four clamped neighbours can be covered
// by different children (a T-junction). A ledge must retain its exposed face.
fn buried_face(set: &BoxSetData, slot: i32, bounds: AABB, point: Vec3, axis: usize, positive: bool) -> bool {
    let slop = linear_slop();
    let tol = seam_tolerance();
    let lo = coords(bounds.lower_bound);
    let hi = coords(bounds.upper_bound);
    let face = if positive { hi[axis] } else { lo[axis] };
    let u = (axis + 1) % 3;
    let v = (axis + 2) % 3;
    let mut q = coords(clamp(point, bounds.lower_bound, bounds.upper_bound));
    q[axis] = face;
    let center = vec3(q[0], q[1], q[2]);
    let samples = [(q[u], q[v]), ((q[u] - slop).max(lo[u]), q[v]), ((q[u] + slop).min(hi[u]), q[v]),
        (q[u], (q[v] - slop).max(lo[v])), (q[u], (q[v] + slop).min(hi[v]))];
    let mut covered = [false; 5];
    set.query(aabb_inflate(AABB { lower_bound: center, upper_bound: center }, slop + tol), &mut |other, child| {
        let nlo = coords(child.bounds.lower_bound);
        let nhi = coords(child.bounds.upper_bound);
        let opposed = if positive { nlo[axis] } else { nhi[axis] };
        if other != slot && (opposed - face).abs() <= tol {
            for (ok, (x, y)) in covered.iter_mut().zip(samples) {
                *ok |= x >= nlo[u] - tol && x <= nhi[u] + tol && y >= nlo[v] - tol && y <= nhi[v] + tol;
            }
        }
        !covered.iter().all(|&hit| hit)
    });
    covered.into_iter().all(|hit| hit)
}

pub(crate) fn exterior_normal(set: &BoxSetData, slot: i32, bounds: AABB, point: Vec3, normal: Vec3) -> Vec3 {
    if set.indexed_box_count() <= 1 {
        return normal;
    }
    let mut n = coords(normal);
    for (axis, component) in n.iter_mut().enumerate() {
        if *component != 0.0 && buried_face(set, slot, bounds, point, axis, *component > 0.0) {
            *component = 0.0;
        }
    }
    if n == coords(normal) {
        return normal;
    }
    let kept = vec3(n[0], n[1], n[2]);
    // Rotating a unit face normal into the other set can leave roundoff on
    // orthogonal axes. It must not become a unit vector when that face drops.
    if length_squared(kept) <= (8.0 * f32::EPSILON).powi(2) {
        Vec3::ZERO
    } else {
        normalize(kept)
    }
}

pub(crate) fn support_plane(bounds: AABB, normal: Vec3) -> Plane {
    let lo = bounds.lower_bound;
    let hi = bounds.upper_bound;
    let support = vec3(
        if normal.x >= 0.0 { hi.x } else { lo.x },
        if normal.y >= 0.0 { hi.y } else { lo.y },
        if normal.z >= 0.0 { hi.z } else { lo.z },
    );
    Plane { normal, offset: dot(normal, support) }
}

fn nearest_box_face(bounds: AABB, point: Vec3) -> (Vec3, f32) {
    let mut nearest = (AXES[0], f32::INFINITY);
    for n in AXES.into_iter().flat_map(|axis| [axis, neg(axis)]) {
        let distance = support_plane(bounds, n).offset - dot(n, point);
        if distance < nearest.1 {
            nearest = (n, distance);
        }
    }
    nearest
}

// Keep the full proxy in its own frame. The pair cast already takes a relative
// pose, so even a large point cloud needs no transformed-point buffer.
pub(crate) fn cast_box_set(
    set: &BoxSetData,
    transform: Transform,
    input: &ShapeCastInput,
    skip_initial: bool,
) -> CastOutput {
    let to_set = invert_transform(transform);
    let start = transform_box_bounds(make_aabb(input.proxy.points, input.proxy.radius), to_set);
    let translation = inv_rotate_vector(transform.q, input.translation);
    let delta = mul_sv(input.max_fraction, translation);
    let end = AABB { lower_bound: add(start.lower_bound, delta), upper_bound: add(start.upper_bound, delta) };
    let mut best = CastOutput::default();
    let mut embedded: Option<EmbeddedWitness> = None;
    // Cached GJK support indices are bytes. Larger query clouds still use all
    // their points, starting a fresh simplex at each advancement.
    let reuse_simplex = input.proxy.points.len() <= usize::from(u8::MAX) + 1;
    set.query(aabb_inflate(aabb_union(start, end), 2.0 * linear_slop()), &mut |slot, child| {
        let vertices = box_vertices(child.bounds);
        let pair = ShapeCastPairInput {
            proxy_a: ShapeProxy { points: &vertices, radius: 0.0 },
            proxy_b: input.proxy,
            transform: to_set,
            translation_b: translation,
            max_fraction: input.max_fraction,
            can_encroach: input.can_encroach,
        };
        let mut hit = if reuse_simplex {
            crate::distance::shape_cast(&pair)
        } else {
            crate::distance::shape_cast_with_simplex_cache::<false>(&pair)
        };
        if !hit.hit || (skip_initial && hit.fraction == 0.0) || (best.hit && hit.fraction > best.fraction) {
            return true;
        }
        if hit.fraction == 0.0 {
            // The generic cast returns a common point and no normal on initial
            // overlap. Recover the two core witnesses before testing child faces.
            let d = crate::distance::shape_distance(
                &DistanceInput { proxy_a: pair.proxy_a, proxy_b: pair.proxy_b, transform: to_set, use_radii: false },
                &mut SimplexCache::default(),
                None,
            );
            let core = match input.proxy.points {
                [single] => transform_point(to_set, *single),
                _ => d.point_b,
            };
            let on_box = clamp(core, child.bounds.lower_bound, child.bounds.upper_bound);
            let delta = sub(core, on_box);
            let (axis, depth) = if delta != Vec3::ZERO {
                (normalize(delta), input.proxy.radius - length(delta))
            } else {
                let (axis, nearest) = nearest_box_face(child.bounds, core);
                (axis, nearest + input.proxy.radius)
            };
            let normal = exterior_normal(set, slot, child.bounds, on_box, axis);
            let other = mul_sub(core, input.proxy.radius, axis);
            if normal == Vec3::ZERO {
                if depth > linear_slop()
                    && embedded.is_none_or(|old| depth > old.depth || (depth == old.depth && slot < old.slot))
                {
                    embedded = Some(EmbeddedWitness { slot, point: other, depth, radius: 0.0 });
                }
                return true;
            }
            hit.normal = normal;
            hit.point = mul_sub(other, dot(normal, other) - support_plane(child.bounds, normal).offset, normal);
        } else {
            let normal = exterior_normal(set, slot, child.bounds, hit.point, hit.normal);
            if normal == Vec3::ZERO || dot(normal, translation) >= 0.0 {
                return true;
            }
            if normal != hit.normal {
                let at_hit = Transform { p: mul_add(to_set.p, hit.fraction, translation), q: to_set.q };
                let d = crate::distance::shape_distance(
                    &DistanceInput {
                        proxy_a: pair.proxy_a,
                        proxy_b: pair.proxy_b,
                        transform: at_hit,
                        use_radii: false,
                    },
                    &mut SimplexCache::default(),
                    None,
                );
                let other = mul_sub(d.point_b, input.proxy.radius, hit.normal);
                let first_fraction = hit.fraction;
                let skin = d.distance - input.proxy.radius;
                let mut axis = normal;
                loop {
                    let approach = -dot(axis, translation);
                    if approach <= 0.0 { return true; }
                    let plane = support_plane(child.bounds, axis);
                    let separation = dot(axis, other) - plane.offset;
                    // Advance this same witness to the cast's original skin.
                    let advance = (separation - skin).max(0.0) / approach;
                    hit.fraction = first_fraction + advance;
                    if hit.fraction > input.max_fraction { return true; }
                    let at_plane = mul_add(other, advance, translation);
                    hit.point = mul_sub(at_plane, dot(axis, at_plane) - plane.offset, axis);
                    hit.normal = axis;
                    if advance == 0.0 { break; }
                    // A delayed hit must still belong to this finite face. If
                    // exposure changed at its new position, every pass removes
                    // an axis, so this needs neither an iteration cap nor scratch.
                    let bounds = aabb_inflate(child.bounds, seam_tolerance());
                    if clamp(hit.point, bounds.lower_bound, bounds.upper_bound) != hit.point { return true; }
                    let next = exterior_normal(set, slot, child.bounds, hit.point, axis);
                    if next == axis { break; }
                    if next == Vec3::ZERO { return true; }
                    axis = next;
                }
            }
        }
        if best.hit && (hit.fraction, slot) >= (best.fraction, best.child_index) {
            return true;
        }
        hit.child_index = slot;
        hit.material_index = i32::try_from(child.material).unwrap_or(NULL_INDEX);
        hit.triangle_index = NULL_INDEX;
        best = hit;
        true
    });
    if let Some(witness) = embedded.filter(|_| !best.hit || best.fraction > 0.0) {
        let exit = exterior_exit(set, witness.slot, witness.point);
        best = CastOutput {
            hit: true,
            fraction: 0.0,
            point: exit.point,
            normal: exit.normal,
            child_index: exit.slot,
            material_index: i32::try_from(box_set_get(set, exit.slot).unwrap().material).unwrap_or(NULL_INDEX),
            triangle_index: NULL_INDEX,
            ..Default::default()
        };
    }
    best.point = transform_point(transform, best.point);
    best.normal = rotate_vector(transform.q, best.normal);
    best
}

#[derive(Clone, Copy)]
struct EmbeddedWitness {
    depth: f32,
    slot: i32,
    point: Vec3,
    radius: f32,
}

/// The shortest axis walk from an interior witness to an uncovered face.
pub(crate) struct SetExit {
    pub point: Vec3,
    pub direction: usize,
    pub normal: Vec3,
    pub distance: f32,
    pub slot: i32,
    pub steps: usize,
}

pub(crate) fn exterior_exit(set: &BoxSetData, slot: i32, mut point: Vec3) -> SetExit {
    // A witness on a shared edge can belong to several children. For each
    // direction start in the one extending farthest along that ray.
    let witness = coords(point);
    let mut starts = [None; 6];
    set.query(AABB { lower_bound: point, upper_bound: point }, &mut |child, value| {
        for (axis, (lo, hi)) in
            coords(value.bounds.lower_bound).into_iter().zip(coords(value.bounds.upper_bound)).enumerate()
        {
            for (direction, end) in [(2 * axis, hi), (2 * axis + 1, -lo)] {
                if starts[direction].is_none_or(|(old, id)| end > old || (end == old && child < id)) {
                    starts[direction] = Some((end, child));
                }
            }
        }
        true
    });
    let bounds = box_set_get(set, slot).unwrap().bounds;
    if starts[0].is_none() {
        // A witness beyond a thin solid can miss every child. Start the walk
        // at the source box, but measure from the true witness below, so an
        // already exterior point does not acquire artificial penetration.
        point = clamp(point, bounds.lower_bound, bounds.upper_bound);
    }
    let origin = coords(point);
    let tol = seam_tolerance();
    let mut best = SetExit { point, direction: 0, normal: AXES[0], distance: f32::INFINITY, slot, steps: 0 };
    for (direction, start) in starts.into_iter().enumerate() {
        let axis = direction / 2;
        let positive = direction % 2 == 0;
        let sign = if positive { 1.0 } else { -1.0 };
        let fallback = if positive { coords(bounds.upper_bound)[axis] } else { -coords(bounds.lower_bound)[axis] };
        let (mut end, mut current) = start.unwrap_or((fallback, slot));
        loop {
            best.steps += 1;
            let mut q = origin;
            q[axis] = sign * end;
            let exit = vec3(q[0], q[1], q[2]);
            let mut next = (end, current);
            set.query(aabb_inflate(AABB { lower_bound: exit, upper_bound: exit }, tol), &mut |id, value| {
                let lo = coords(value.bounds.lower_bound)[axis];
                let hi = coords(value.bounds.upper_bound)[axis];
                let (near, far) = if positive { (lo, hi) } else { (-hi, -lo) };
                // The query checks both in-plane coordinates (closed, within
                // seam tolerance). Strict progress also handles very thin boxes.
                if id != current
                    && (near - end).abs() <= tol
                    && far > end
                    && (far > next.0 || (far == next.0 && id < next.1))
                {
                    next = (far, id);
                }
                true
            });
            if next.0 == end {
                let distance = end - sign * witness[axis];
                if distance < best.distance {
                    best.point = exit;
                    best.distance = distance;
                    best.normal = mul_sv(sign, AXES[axis]);
                    best.direction = direction;
                    best.slot = current;
                }
                break;
            }
            (end, current) = next;
        }
    }
    best
}

#[derive(Default)]
struct MoverScratch {
    candidates: Vec<(i32, PlaneResult)>,
    planes: Vec<PlaneResult>,
}

thread_local! {
    // The callback can issue another query on this thread. Each active query
    // owns a buffer until its callback returns.
    static MOVER_SCRATCH: std::cell::RefCell<Vec<MoverScratch>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn with_box_set_mover_planes<R>(
    set: &BoxSetData, transform: Transform, mover: &Capsule, visit: impl FnOnce(&[PlaneResult]) -> R,
) -> R {
    let mut scratch = MOVER_SCRATCH.with(|pool| pool.borrow_mut().pop().unwrap_or_default());
    collect_mover_planes(&mut scratch, set, transform, mover);
    let result = visit(&scratch.planes);
    MOVER_SCRATCH.with(|pool| pool.borrow_mut().push(scratch));
    result
}

fn collect_mover_planes(scratch: &mut MoverScratch, set: &BoxSetData, transform: Transform, mover: &Capsule) {
    let points = [inv_transform_point(transform, mover.center1), inv_transform_point(transform, mover.center2)];
    let segment = sub(points[1], points[0]);
    scratch.candidates.clear();
    scratch.planes.clear();
    let mut embedded: Option<EmbeddedWitness> = None;
    set.query(make_aabb(&points, mover.radius), &mut |slot, child| {
        let vertices = box_vertices(child.bounds);
        let distance = crate::distance::shape_distance(
            &DistanceInput {
                proxy_a: ShapeProxy { points: &vertices, radius: 0.0 },
                proxy_b: ShapeProxy { points: &points, radius: mover.radius },
                transform: Transform::IDENTITY,
                use_radii: false,
            },
            &mut SimplexCache::default(),
            None,
        );
        if distance.distance > mover.radius {
            return true;
        }
        // If the core intersects the box, a GJK common point need not be
        // centred on the clipped segment. Take its midpoint for a stable exit.
        let (core, original, depth) = if distance.distance == 0.0 {
            let ray = RayCastInput { origin: points[0], translation: segment, max_fraction: 1.0 };
            let back = RayCastInput { origin: points[1], translation: neg(segment), max_fraction: 1.0 };
            let core = match (ray_entry(child.bounds, &ray), ray_entry(child.bounds, &back)) {
                (Some((a, _)), Some((b, _))) => mul_add(points[0], 0.5 * (a + (1.0 - b)), segment),
                _ => distance.point_b,
            };
            let (axis, to_face) = nearest_box_face(child.bounds, core);
            (core, axis, mover.radius + to_face)
        } else {
            let core = distance.point_b;
            let closest = clamp(core, child.bounds.lower_bound, child.bounds.upper_bound);
            (core, normalize(sub(core, closest)), mover.radius - distance.distance)
        };
        let other = mul_sub(core, mover.radius, original);
        let on_box = if distance.distance == 0.0 {
            mul_add(other, depth, original)
        } else {
            clamp(core, child.bounds.lower_bound, child.bounds.upper_bound)
        };
        let normal = exterior_normal(set, slot, child.bounds, on_box, original);
        if normal != Vec3::ZERO {
            let offset = dot(normal, sub(on_box, other));
            let point = if normal == original { on_box } else { mul_add(other, offset, normal) };
            scratch.candidates.push((slot, PlaneResult { point, plane: Plane { normal, offset } }));
        } else if depth > linear_slop()
            && embedded.is_none_or(|old| depth > old.depth || (depth == old.depth && slot < old.slot))
        {
            // A core inside the solid must clear the exit plane by its radius.
            let (point, radius) = if distance.distance == 0.0 { (core, mover.radius) } else { (other, 0.0) };
            embedded = Some(EmbeddedWitness { depth, slot, point, radius });
        }
        true
    });

    scratch.candidates.sort_unstable_by_key(|&(slot, _)| slot);
    for &(_, candidate) in &scratch.candidates {
        let same = scratch.planes.iter_mut().find(|old| {
            dot(old.plane.normal, candidate.plane.normal) > 0.996
                && dot(old.plane.normal, sub(candidate.point, old.point)).abs() <= linear_slop()
        });
        if let Some(old) = same {
            if candidate.plane.offset > old.plane.offset {
                *old = candidate;
            }
        } else {
            scratch.planes.push(candidate);
        }
    }
    if scratch.planes.is_empty() {
        if let Some(witness) = embedded {
            let exit = exterior_exit(set, witness.slot, witness.point);
            scratch.planes.push(PlaneResult {
                point: exit.point,
                plane: Plane { normal: exit.normal, offset: exit.distance + witness.radius },
            });
        }
    }
    for hit in &mut scratch.planes {
        hit.point = transform_point(transform, hit.point);
        hit.plane.normal = rotate_vector(transform.q, hit.plane.normal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_recycles_slots_and_preserves_shared_geometry() {
        let boxes = [0, 1, 2].map(|x| SetBox {
            bounds: AABB { lower_bound: vec3(x as f32, 0.0, 0.0), upper_bound: vec3(x as f32 + 1.0, 1.0, 1.0) },
            material: x,
            user_data: 100 + x as u64,
        });
        let original = create_box_set(&boxes);
        let mut edited = Arc::clone(&original);
        let set = Arc::make_mut(&mut edited);
        let ray = RayCastInput { origin: vec3(1.5, -1.0, 0.5), translation: vec3(0.0, 4.0, 0.0), max_fraction: 1.0 };
        let mut added = Vec::new();
        set.edit(&[1], &[], &mut added);
        assert_eq!(set.serial(1), None, "a removed child must have no live serial");
        assert!(!ray_cast_box_set(set, &ray).hit, "the ray must pass through the carved opening");
        let original_hit = ray_cast_box_set(&original, &ray);
        assert!(original_hit.hit && original_hit.child_index == 1, "the shared original must still block the ray");
        assert_eq!(set.boxes().map(|(i, _)| i).collect::<Vec<_>>(), [0, 2], "a hole must not renumber later children");
        let remaining = compute_box_set_mass(set, 6.0);
        assert_eq!(remaining.mass, 12.0, "removal must subtract the child's volume");
        assert_eq!(remaining.inertia.cy.y, 14.0, "the surviving cubes must keep their separation inertia");

        let replacement = SetBox { user_data: u64::MAX, ..boxes[1] };
        set.edit(&[], &[replacement], &mut added);
        assert_eq!(added, [1], "reinsertion must return the freed slot");
        assert_ne!(set.serial(1), original.serial(1), "a reused slot must have a fresh serial");
        assert_eq!(set.serial(2), original.serial(2), "a surviving slot must keep its serial");
        assert_eq!(set.revision, original.revision + 2, "each effective edit must advance the set revision once");
        assert_eq!(box_set_get(set, 1), Some(&replacement), "lookup must preserve the full replacement payload");
        assert_eq!(box_set_get(&original, 1), Some(&boxes[1]), "copy-on-write must preserve the original payload");

        set.edit(&[0, 1, 2], &[], &mut added);
        assert!(added.is_empty(), "the added output must only describe the latest edit");
        assert_eq!(set.bounds(), None, "the last removal must clear the tree bounds");
        assert_eq!(compute_box_set_mass(set, 6.0).mass, 0.0, "an empty set must have zero mass");
        set.edit(&[], &boxes[..1], &mut added);
        assert_eq!(set.bounds(), Some(boxes[0].bounds), "refilling an empty set must restore its bounds");
        assert_eq!(compute_box_set_mass(set, 6.0).center, vec3(0.5, 0.5, 0.5), "refill must start with fresh moments");

        let mut wire = RecBuffer::new();
        set.write_snapshot(&mut wire);
        let restored = BoxSetData::read_snapshot(&mut RecCursor::new(&wire.data)).expect("edited tree must decode");
        assert_eq!(restored.revision, set.revision, "the snapshot must preserve the edit revision");
        assert!(restored.slots.iter().zip(&set.slots).all(|(a, b)| a.serial == b.serial),
            "both live and free slot lifetimes must survive a snapshot");
        let free = set.tree.free_list;
        set.tree.nodes[free as usize].parent = free;
        let mut bad = RecBuffer::new();
        set.write_snapshot(&mut bad);
        assert!(BoxSetData::read_snapshot(&mut RecCursor::new(&bad.data)).is_none(), "a cyclic node free list must fail to load");
    }

    #[test]
    fn removing_dominant_mass_keeps_small_survivors() {
        let cube =
            SetBox { bounds: AABB { lower_bound: Vec3::ZERO, upper_bound: Vec3::ONE }, material: 0, user_data: 0 };
        let mut boxes = [cube; 3];
        boxes[0].bounds.upper_bound = vec3(1048576.0, 1048576.0, 1048576.0);
        boxes[1].bounds = AABB { lower_bound: vec3(-2.0, 0.0, 0.0), upper_bound: vec3(-1.0, 1.0, 1.0) };
        boxes[2].bounds = AABB { lower_bound: vec3(-1.0, 0.0, 0.0), upper_bound: vec3(0.0, 1.0, 1.0) };
        let mut set = create_box_set(&boxes);
        Arc::make_mut(&mut set).edit(&[0], &[], &mut Vec::new());
        let mass = compute_box_set_mass(&set, 6.0);
        assert_eq!(mass.mass, 12.0, "subtracting a volume of 2^60 must preserve both unit cubes");
        assert_eq!(mass.center, vec3(-1.0, 0.5, 0.5), "the survivors must retain their common center of mass");
        assert_eq!(
            (mass.inertia.cx.x, mass.inertia.cy.y, mass.inertia.cz.z),
            (2.0, 5.0, 5.0),
            "the surviving 2 by 1 by 1 solid must retain its central inertia"
        );
    }
}
