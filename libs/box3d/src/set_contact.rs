//! Contacts with an editable box set. Triangle surfaces are A against set B;
//! a set is A against a convex shape or another set. Child lifetimes belong to the shape pair.
use crate::box_set::{box_set_get, exterior_exit, exterior_normal, transform_box_bounds, transform_box_bounds_by, BoxSetData};
use crate::aabb::perimeter;
use crate::dynamic_tree::{dynamic_tree_get_height, is_leaf};
use crate::constants::{linear_slop, max_aabb_margin, mesh_rest_offset, speculative_distance, MAX_MANIFOLD_POINTS};
use crate::contact::{mix_friction, mix_restitution, mix_yield_force, Contact, ContactCache, Manifolds};
use crate::convex_manifold::{collide_hull_and_capsule, collide_hull_and_sphere, collide_hulls};
use crate::core::NULL_INDEX;
use crate::manifold::{make_feature_id, FeatureOwner};
use crate::math_functions::*;
use crate::math_internal::{distance_squared2, make_plane_from_points, plane_separation, Triangle};
use crate::mesh_contact::{cull_points, Point2D};
use crate::physics_world::{TaskContext, World};
use crate::recording::RecBuffer;
use crate::recording_replay::RecCursor;
use crate::shape::{get_shape_materials, Shape, ShapeGeometry};
use crate::types::{
    HullData, LocalManifold, LocalManifoldPoint, Manifold, SATCache, SeparatingFeature, SurfaceMaterial, TreeNode, TriangleFeature,
    ALL_FLAT_EDGES, DEFAULT_MASK_BITS, FLAT_EDGE1, FLAT_EDGE2, FLAT_EDGE3,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
struct Row {
    slot_a: i32,
    serial_a: u32,
    slot_b: i32,
    serial_b: u32,
}

#[derive(Clone, Copy, Debug)]
struct ChildPair {
    // For a triangle surface the triangle index is its child ordering key.
    // The public row still has NULL_INDEX on that side.
    children: (i32, i32),
    serials: (u32, u32),
    triangle: usize,
    cache: ContactCache,
}

#[derive(Clone, Copy, Debug)]
struct SurfaceTriangle {
    index: i32,
    geometry: Triangle,
    plane: Plane,
    material: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct Patch {
    plane: Plane,
    materials: [usize; 2],
    friction: f32,
    restitution: f32,
    yield_force: f32,
    rolling_resistance: f32,
    tangent_velocity: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct SetContact {
    pairs: Vec<ChildPair>,
    // Rebuild in contact-owned storage so a worker switch or a different
    // contact's query cannot take either buffer's capacity.
    next_pairs: Vec<ChildPair>,
    // Padded query in A's frame: cached surface triangles or cached set children.
    query_bounds: Option<AABB>,
    triangles: Vec<SurfaceTriangle>,
    revisions: [u64; 2],
    rows: Vec<Row>,
    prev_rows: Vec<Row>,
    patches: Vec<Patch>,
    spare_manifolds: Vec<Manifold>,
}

fn revision(shape: &Shape) -> u64 {
    match &shape.geom {
        ShapeGeometry::BoxSet(set) => set.revision,
        _ => 0,
    }
}

fn live_child(shape: &Shape, slot: i32, serial: u32) -> bool {
    match &shape.geom {
        ShapeGeometry::BoxSet(set) => set.serial(slot) == Some(serial),
        _ => slot == NULL_INDEX,
    }
}

fn material_index(material: u32, count: usize) -> usize {
    (i32::try_from(material).unwrap_or(0) as usize).min(count - 1)
}

// Bound allocations by the remaining wire bytes before reserving any cache.
fn snapshot_count(r: &mut RecCursor, item_bytes: usize) -> Option<usize> {
    let count = usize::try_from(r.r_u64()).ok()?;
    (r.ok && count <= r.remaining() / item_bytes).then_some(count)
}

impl SetContact {
    pub(crate) fn write_snapshot(&self, buf: &mut RecBuffer) {
        for &rev in &self.revisions {
            buf.w_u64(rev);
        }
        buf.w_bool(self.query_bounds.is_some());
        if let Some(bounds) = self.query_bounds {
            buf.w_aabb(bounds);
        }
        buf.w_u64(self.triangles.len() as u64);
        for t in &self.triangles {
            buf.w_i32(t.index);
            for vertex in t.geometry.vertices {
                buf.w_vec3(vertex);
            }
            for i in [t.geometry.i1, t.geometry.i2, t.geometry.i3, t.geometry.flags] {
                buf.w_i32(i);
            }
            buf.w_vec3(t.plane.normal);
            buf.w_f32(t.plane.offset);
            buf.w_u32(t.material as u32);
        }
        buf.w_u64(self.pairs.len() as u64);
        for pair in &self.pairs {
            buf.w_i32(pair.children.0);
            buf.w_i32(pair.children.1);
            buf.w_u32(pair.serials.0);
            buf.w_u32(pair.serials.1);
            buf.w_u64(pair.triangle as u64);
            crate::world_snapshot::ser_contact_cache(buf, &pair.cache);
        }
        for rows in [&self.rows, &self.prev_rows] {
            buf.w_u64(rows.len() as u64);
            for row in rows {
                buf.w_i32(row.slot_a);
                buf.w_u32(row.serial_a);
                buf.w_i32(row.slot_b);
                buf.w_u32(row.serial_b);
            }
        }
        buf.w_u64(self.patches.len() as u64);
        for p in &self.patches {
            buf.w_vec3(p.plane.normal);
            buf.w_f32(p.plane.offset);
            for &material in &p.materials {
                buf.w_u32(material as u32);
            }
            buf.w_f32(p.friction);
            buf.w_f32(p.restitution);
            buf.w_f32(p.yield_force);
            buf.w_f32(p.rolling_resistance);
            buf.w_vec3(p.tangent_velocity);
        }
        // next_pairs and spare_manifolds are empty workspace. The live
        // pairs are above and the live manifolds precede this payload.
    }

    pub(crate) fn read_snapshot(r: &mut RecCursor, manifolds: &[Manifold]) -> Option<Self> {
        let revisions = [r.r_u64(), r.r_u64()];
        let query_bounds = match r.r_u8() {
            0 => None,
            1 => Some(r.r_aabb()),
            _ => return None,
        };
        let count = snapshot_count(r, 76)?;
        let mut triangles = Vec::with_capacity(count);
        for _ in 0..count {
            triangles.push(SurfaceTriangle {
                index: r.r_i32(),
                geometry: Triangle {
                    vertices: std::array::from_fn(|_| r.r_vec3()),
                    i1: r.r_i32(),
                    i2: r.r_i32(),
                    i3: r.r_i32(),
                    flags: r.r_i32(),
                },
                plane: Plane { normal: r.r_vec3(), offset: r.r_f32() },
                material: r.r_u32() as usize,
            });
        }
        let count = snapshot_count(r, 76)?;
        let mut pairs = Vec::with_capacity(count);
        for _ in 0..count {
            let children = (r.r_i32(), r.r_i32());
            let serials = (r.r_u32(), r.r_u32());
            let triangle = usize::try_from(r.r_u64()).ok()?;
            if serials.0 == 0 && triangles.get(triangle)?.index != children.0 {
                return None;
            }
            let cache = crate::world_snapshot::des_contact_cache(r);
            if usize::from(cache.simplex_cache.count) > cache.simplex_cache.index_a.len() {
                return None;
            }
            pairs.push(ChildPair { children, serials, triangle, cache });
        }
        let mut read_rows = || -> Option<Vec<Row>> {
            let count = snapshot_count(r, 16)?;
            if count % MAX_MANIFOLD_POINTS != 0 {
                return None;
            }
            let rows = (0..count).map(|_| Row {
                slot_a: r.r_i32(),
                serial_a: r.r_u32(),
                slot_b: r.r_i32(),
                serial_b: r.r_u32(),
            }).collect();
            Some(rows)
        };
        let rows = read_rows()?;
        let prev_rows = read_rows()?;
        let count = snapshot_count(r, 48)?;
        if rows.len() / MAX_MANIFOLD_POINTS != manifolds.len()
            || count != manifolds.len()
            || manifolds.iter().any(|m| !(0..=MAX_MANIFOLD_POINTS as i32).contains(&m.point_count))
        {
            return None;
        }
        let mut patches = Vec::with_capacity(count);
        for _ in 0..count {
            patches.push(Patch {
                plane: Plane { normal: r.r_vec3(), offset: r.r_f32() },
                materials: [r.r_u32() as usize, r.r_u32() as usize],
                friction: r.r_f32(),
                restitution: r.r_f32(),
                yield_force: r.r_f32(),
                rolling_resistance: r.r_f32(),
                tangent_velocity: r.r_vec3(),
            });
        }
        r.ok.then_some(Self { revisions, query_bounds, triangles, pairs, rows, prev_rows, patches, ..Self::default() })
    }

    pub(crate) fn material_coefficients(&self, manifold: usize) -> (f32, f32, f32) {
        let patch = &self.patches[manifold];
        (patch.friction, patch.restitution, patch.rolling_resistance)
    }

    pub(crate) fn material_indices(&self, manifold: usize) -> [usize; 2] {
        self.patches[manifold].materials
    }

    pub(crate) fn revisions_match(&self, a: &Shape, b: &Shape) -> bool {
        self.revisions == [revision(a), revision(b)]
    }

    pub(crate) fn invalidate_surface_query(&mut self) {
        // Cached triangle indices were clamped to the mesh's previous material count.
        self.query_bounds = None;
    }

    pub(crate) fn point_children(&self, a: &Shape, b: &Shape, manifold: usize, point: usize) -> Option<(i32, i32)> {
        let row = self.rows.get(manifold * MAX_MANIFOLD_POINTS + point)?;
        (live_child(a, row.slot_a, row.serial_a) && live_child(b, row.slot_b, row.serial_b))
            .then_some((row.slot_a, row.slot_b))
    }

    pub(crate) fn recycle(&mut self, manifolds: &mut Manifolds) {
        self.resize_manifolds(manifolds, 0);
        self.query_bounds = None;
        self.revisions = [0; 2];
        self.triangles.clear();
        self.pairs.clear();
        self.patches.clear();
        self.rows.clear();
        self.prev_rows.clear();
    }

    fn resize_manifolds(&mut self, manifolds: &mut Manifolds, count: usize) {
        if let Manifolds::Many(mut v) = std::mem::take(manifolds) {
            v.clear();
            self.spare_manifolds = v;
        }
        *manifolds = match count {
            0 => Manifolds::None,
            1 => Manifolds::One(Manifold::default()),
            _ => {
                let mut v = std::mem::take(&mut self.spare_manifolds);
                v.resize(count, Manifold::default());
                Manifolds::Many(v)
            }
        };
    }

    fn refresh_triangles(&mut self, surface: &Shape, set: &BoxSetData, b_to_a: Transform) {
        let revision = [0, set.revision];
        let Some(bounds) = set.bounds() else {
            self.triangles.clear();
            self.query_bounds = None;
            self.revisions = revision;
            return;
        };
        let bounds = aabb_inflate(transform_box_bounds(bounds, b_to_a), speculative_distance());
        if self.revisions == revision && self.query_bounds.is_some_and(|old| aabb_contains(old, bounds)) {
            return;
        }
        let query = aabb_inflate(bounds, max_aabb_margin());
        self.query_bounds = Some(query);
        self.revisions = revision;
        self.triangles.clear();
        let last_material = get_shape_materials(surface).len() - 1;
        let mut save = |index: i32, geometry: Triangle, material: usize| {
            let [a, b, c] = geometry.vertices;
            self.triangles.push(SurfaceTriangle {
                index,
                geometry,
                plane: make_plane_from_points(a, b, c),
                material: material.min(last_material),
            });
            true
        };
        match &surface.geom {
            ShapeGeometry::Mesh(mesh) => crate::mesh::query_mesh(mesh, query, &mut |_, _, _, i| {
                let material = mesh.data.materials.get(i as usize).copied().unwrap_or(0);
                save(i, crate::mesh::get_mesh_triangle(mesh, i), material as usize)
            }),
            ShapeGeometry::HeightField(field) => {
                if !aabb_overlaps(query, field.aabb) {
                    return;
                }
                let query = AABB {
                    lower_bound: max(query.lower_bound, field.aabb.lower_bound),
                    upper_bound: min(query.upper_bound, field.aabb.upper_bound),
                };
                crate::height_field::query_height_field(field, query, &mut |_, _, _, i| {
                    save(
                        i,
                        crate::height_field::get_height_field_triangle(field, i),
                        crate::height_field::get_height_field_material(field, i) as usize,
                    )
                })
            }
            _ => unreachable!("set contacts require a registered triangle surface"),
        }
    }
}

// Fixed box topology is shared across all child pairs on a worker.
fn scratch_box(storage: &mut Option<HullData>) -> &mut HullData {
    storage.get_or_insert_with(|| std::sync::Arc::try_unwrap(crate::hull::make_box_hull(1.0, 1.0, 1.0)).unwrap())
}

// Use the actual endpoints even when the midpoint of a thin interval rounds to a face.
fn write_box(hull: &mut HullData, bounds: AABB) {
    let lo = bounds.lower_bound;
    let hi = bounds.upper_bound;
    hull.aabb = bounds;
    hull.center = add(mul_sv(0.5, lo), mul_sv(0.5, hi));
    hull.points.copy_from_slice(&crate::box_set::box_vertices(bounds));
    for (plane, offset) in hull.planes.iter_mut().zip([-lo.x, hi.x, -lo.y, hi.y, -lo.z, hi.z]) {
        plane.offset = offset;
    }
    let h = aabb_extents(bounds);
    hull.inner_radius = h.x.min(h.y).min(h.z);
}

#[derive(Clone, Copy, Debug, Default)]
struct SetPoint {
    geometry: LocalManifoldPoint,
    row: Row,
    rolling_radius: f32,
    corrected: bool,
}

impl SetPoint {
    // Measure the original witness pair along the corrected contact normal.
    fn project_separation(&mut self, old: Vec3, normal: Vec3) {
        let separation = self.geometry.separation;
        let projected = separation * dot(old, normal).min(1.0);
        debug_assert!(projected.abs() <= separation.abs());
        self.geometry.separation = projected;
        self.corrected = true;
    }
}

#[derive(Clone, Debug)]
struct FaceContact {
    pair_index: usize,
    triangle: usize,
    slot: i32,
    material: usize,
    normal: Vec3,
    accepted: bool,
    midpoint: bool,
    points: std::ops::Range<usize>,
}

#[derive(Clone, Debug, Default)]
struct Cluster {
    normal: Vec3,
    patch: Patch,
    points: Vec<SetPoint>,
}

// Ordinary feature owners are 0 and 1. The low byte identifies the walk direction.
const RECOVERY_OWNER: u8 = u8::MAX;

type PointKey = (Row, i32, u32);
#[derive(Clone, Copy, Debug)]
struct OldPoint {
    key: PointKey,
    manifold: usize,
    point: usize,
    impulse: f32,
    normal: Vec3,
    claimed: bool,
}

#[derive(Clone, Copy, Debug)]
struct PatchImpulses {
    friction: Vec3,
    rolling: Vec3,
    twist: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SetCollideScratch {
    // Each set uses the scratch hull for its own side of the contact.
    hulls: [Option<HullData>; 2],
    vertices: Vec<[Vec3; 3]>,
    local: LocalManifold,
    faces: Vec<FaceContact>,
    points: Vec<SetPoint>,
    // A triangle's edge can only suppress a ghost on the same box.
    edges: Vec<(i32, i32, i32)>,
    clusters: Vec<Cluster>,
    projected: Vec<Point2D>,
    old_points: Vec<OldPoint>,
    old_impulses: Vec<Option<PatchImpulses>>,
    deepest_drop: Option<DroppedPoint>,
    // Set/set descent: (outer node, inner node, outer node's bounds as the inner side tests them).
    descent: Vec<(i32, i32, AABB)>,
    triangle_stack: Vec<i32>,
}

fn triangle_child_pairs(cache: &mut SetContact, scratch: &mut SetCollideScratch, set: &BoxSetData, a_to_b: Transform) -> usize {
    scratch.vertices.clear();
    let tree = set.tree();
    let stack = &mut scratch.triangle_stack;
    stack.clear();
    if tree.root == NULL_INDEX {
        update_pairs(cache);
        return 0;
    }
    stack.reserve((dynamic_tree_get_height(tree) + 2) as usize);
    let margin = speculative_distance();
    let limit = margin + linear_slop();
    let mut visits = 0;
    let pairs = &mut cache.next_pairs;
    for (i, triangle) in cache.triangles.iter().enumerate() {
        let [a, b, c] = triangle.geometry.vertices.map(|p| transform_point(a_to_b, p));
        scratch.vertices.push([a, b, c]);
        let bounds = AABB { lower_bound: min(a, min(b, c)), upper_bound: max(a, max(b, c)) };
        let query = aabb_inflate(bounds, margin);
        let plane = make_plane_from_points(a, b, c);
        let n = plane.normal;
        stack.push(tree.root);
        while let Some(id) = stack.pop() {
            visits += 1;
            let node = &tree.nodes[id as usize];
            if node.category_bits & DEFAULT_MASK_BITS == 0 || !aabb_overlaps(node.aabb, query) {
                continue;
            }
            // If the lowest corner clears the plane, every box below this node clears it too.
            let lo = node.aabb.lower_bound;
            let hi = node.aabb.upper_bound;
            let p = vec3(
                if n.x >= 0.0 { lo.x } else { hi.x },
                if n.y >= 0.0 { lo.y } else { hi.y },
                if n.z >= 0.0 { lo.z } else { hi.z },
            );
            if plane_separation(plane, p) > limit {
                continue;
            }
            if is_leaf(node) {
                let slot = node.user_data as i32;
                pairs.push(ChildPair {
                    children: (triangle.index, slot),
                    serials: (0, set.serial(slot).unwrap()),
                    triangle: i,
                    cache: ContactCache::default(),
                });
            } else {
                stack.push(node.children.child1);
                stack.push(node.children.child2);
            }
        }
    }
    update_pairs(cache);
    visits
}

fn convex_child_pairs(cache: &mut SetContact, set: &BoxSetData, bounds: AABB) -> usize {
    let revisions = [set.revision, 0];
    if cache.revisions == revisions && cache.query_bounds.is_some_and(|old| aabb_contains(old, bounds)) {
        return 0;
    }
    let query = aabb_inflate(bounds, max_aabb_margin());
    let pairs = &mut cache.next_pairs;
    set.query(query, &mut |slot, _| {
        pairs.push(ChildPair {
            children: (slot, NULL_INDEX),
            serials: (set.serial(slot).unwrap(), 0),
            triangle: 0, // Only the surface path indexes the triangle buffer.
            cache: ContactCache::default(),
        });
        true
    });
    update_pairs(cache);
    cache.revisions = revisions;
    cache.query_bounds = Some(query);
    cache.pairs.len() // One candidate per visited box.
}

fn set_child_pairs(
    cache: &mut SetContact,
    stack: &mut Vec<(i32, i32, AABB)>,
    a: &BoxSetData,
    b: &BoxSetData,
    a_to_b: Transform,
    b_to_a: Transform,
    margin: f32,
) -> usize {
    let outer_is_a = a.indexed_box_count() <= b.indexed_box_count();
    let (outer, inner, to_outer, to_inner) =
        if outer_is_a { (a, b, b_to_a, a_to_b) } else { (b, a, a_to_b, b_to_a) };
    let pairs = &mut cache.next_pairs;
    let mut tested = 0;
    let (outer_tree, inner_tree) = (outer.tree(), inner.tree());
    if let (Some(bounds), true) = (inner.bounds(), outer_tree.root != NULL_INDEX) {
        // A candidate is an outer box that meets the inner set's padded bounds,
        // with an inner box that meets the outer box's padded bounds in the inner
        // frame. Both trees are descended together: a node contains its leaves
        // and the transform is monotone, so a node pair that fails either test
        // cannot hold a candidate, and the leaf test is the per-box query's own.
        // The pair set does not depend on how far the bodies overlap.
        let query = aabb_inflate(transform_box_bounds(bounds, to_outer), margin);
        let rotation = make_matrix_from_quat(to_inner.q);
        let seen = |node: &TreeNode| aabb_inflate(transform_box_bounds_by(node.aabb, rotation, to_inner.p), margin);
        let meets = |node: &TreeNode, bounds: AABB| node.category_bits & DEFAULT_MASK_BITS != 0 && aabb_overlaps(node.aabb, bounds);
        // Each pop pushes at most two pairs one level down either tree.
        stack.clear();
        stack.reserve((dynamic_tree_get_height(outer_tree) + dynamic_tree_get_height(inner_tree) + 2) as usize);
        let root = &outer_tree.nodes[outer_tree.root as usize];
        if meets(root, query) {
            stack.push((outer_tree.root, inner_tree.root, seen(root)));
        }
        while let Some((o, i, outer_seen)) = stack.pop() {
            tested += 1;
            let inner_node = &inner_tree.nodes[i as usize];
            if !meets(inner_node, outer_seen) {
                continue;
            }
            let outer_node = &outer_tree.nodes[o as usize];
            let (outer_leaf, inner_leaf) = (is_leaf(outer_node), is_leaf(inner_node));
            if outer_leaf && inner_leaf {
                let (outer_slot, inner_slot) = (outer_node.user_data as i32, inner_node.user_data as i32);
                let (outer_serial, inner_serial) = (outer.serial(outer_slot).unwrap(), inner.serial(inner_slot).unwrap());
                let (children, serials) = if outer_is_a {
                    ((outer_slot, inner_slot), (outer_serial, inner_serial))
                } else {
                    ((inner_slot, outer_slot), (inner_serial, outer_serial))
                };
                pairs.push(ChildPair { children, serials, triangle: 0, cache: ContactCache::default() });
            } else if inner_leaf || (!outer_leaf && perimeter(outer_node.aabb) >= perimeter(inner_node.aabb)) {
                for child in [outer_node.children.child1, outer_node.children.child2] {
                    let node = &outer_tree.nodes[child as usize];
                    if meets(node, query) {
                        stack.push((child, i, seen(node)));
                    }
                }
            } else {
                stack.push((o, inner_node.children.child1, outer_seen));
                stack.push((o, inner_node.children.child2, outer_seen));
            }
        }
    }
    // Internal overlaps can change under rotation even if the root bounds stay put.
    update_pairs(cache);
    cache.revisions = [a.revision, b.revision];
    tested + cache.pairs.len() // Node pairs tested, then each emitted pair.
}

fn update_pairs(cache: &mut SetContact) {
    let pairs = &mut cache.next_pairs;
    pairs.sort_unstable_by_key(|p| p.children);
    let mut old = cache.pairs.iter().peekable();
    for new in pairs.iter_mut() {
        while old.peek().is_some_and(|p| p.children < new.children) {
            old.next();
        }
        if let Some(p) = old.next_if(|p| p.children == new.children) {
            if p.serials == new.serials {
                new.cache = p.cache;
            }
        }
    }
    std::mem::swap(&mut cache.pairs, pairs);
    pairs.clear();
    // Warm both banks at a new peak, including the first non-empty query.
    pairs.reserve(cache.pairs.len());
}

fn edges(slot: i32, t: Triangle) -> [(i32, i32, i32); 3] {
    [(t.i1, t.i2), (t.i2, t.i3), (t.i3, t.i1)].map(|(a, b)| (slot, a.min(b), a.max(b)))
}

fn collide_triangle_pairs(
    cache: &mut SetContact,
    scratch: &mut SetCollideScratch,
    set: &BoxSetData,
    material_count: usize,
    is_fast: bool,
) -> i32 {
    scratch.faces.clear();
    scratch.points.clear();
    scratch.edges.clear();
    if cache.pairs.is_empty() {
        return 0;
    }
    let hull = scratch_box(&mut scratch.hulls[1]);
    let mut hits = 0;
    for (pair_index, pair) in cache.pairs.iter_mut().enumerate() {
        let (triangle_index, slot) = pair.children;
        let triangle = cache.triangles[pair.triangle];
        let child = box_set_get(set, slot).unwrap();
        write_box(hull, child.bounds);
        let [a, b, c] = scratch.vertices[pair.triangle];
        let sat = &mut pair.cache.sat_cache;
        if is_fast && sat.type_ == SeparatingFeature::EdgePairAxis as u8 {
            *sat = SATCache::default();
        }
        let local = &mut scratch.local;
        crate::triangle_manifold::collide_hull_and_triangle(
            local,
            i32::MAX,
            hull,
            a,
            b,
            c,
            triangle.geometry.flags,
            sat,
        );
        hits += i32::from(sat.hit);
        if local.points.is_empty() {
            continue;
        }

        // Same hull/terrain ghost-edge rule as mesh contacts. Mesh adjacency
        // applies within one child; exposed faces of adjacent boxes are separate.
        let triangle_normal = crate::math_internal::make_normal_from_points(a, b, c);
        let accepted = crate::contact::FORCE_GHOST_COLLISIONS
            || local.feature == TriangleFeature::TriangleFace
            || (local.feature == TriangleFeature::HullFace
                && (dot(triangle_normal, local.normal) > 0.5
                    || local.points.iter().any(|p| p.separation < -2.0 * linear_slop())));
        if accepted {
            scratch.edges.extend(edges(slot, triangle.geometry));
        }
        let start = scratch.points.len();
        let row = Row { slot_a: NULL_INDEX, serial_a: 0, slot_b: slot, serial_b: pair.serials.1 };
        scratch.points.extend(local.points.iter().map(|p| SetPoint {
            geometry: LocalManifoldPoint { triangle_index, ..*p },
            row,
            rolling_radius: hull.inner_radius,
            corrected: false,
        }));
        scratch.faces.push(FaceContact {
            pair_index,
            triangle: pair.triangle,
            slot,
            normal: local.normal,
            accepted,
            // Triangle face clipping stores the hull witness, as does the GJK
            // fallback. Only the SAT edge pair stores the midpoint.
            midpoint: matches!(local.feature, TriangleFeature::Edge1 | TriangleFeature::Edge2 | TriangleFeature::Edge3)
                && local.points[0].pair != crate::manifold::FEATURE_PAIR_SINGLE,
            material: material_index(child.material, material_count),
            points: start..scratch.points.len(),
        });
    }
    scratch.edges.sort_unstable();
    scratch.edges.dedup();
    hits
}

// A copy of the mesh reduction over set points. The common planar cull returns
// indices, so one assignment carries geometry and both child lifetimes together.
fn reduce_cluster(points: &mut Vec<SetPoint>, projected: &mut Vec<Point2D>, normal: Vec3) {
    if points.len() <= 1 {
        return;
    }
    projected.clear();
    let u = perp(normal);
    let v = cross(normal, u);
    let origin = points[0].geometry.point;
    projected.extend(points.iter().enumerate().map(|(i, p)| {
        let d = sub(p.geometry.point, origin);
        Point2D { p: vec2(dot(d, u), dot(d, v)), separation: p.geometry.separation, original_index: i as i32 }
    }));
    let count = cull_points(projected) as usize;
    // The cull keeps the widest points and calls depths within a slop equal.
    // A cluster can span a whole set (a felled trunk's two ends), so the lowest
    // corner of a tilted box near one end loses to a corner a few millimetres
    // higher beside it. The solver then rests the set on the higher corner and,
    // whenever a later cull lets the lower one through, pushes it out by the
    // difference: a resting set gains energy and never sleeps. So each kept
    // point stands for its region (the points nearer to it than to any other
    // kept point). When the region holds a deeper point, the kept point gives
    // way to the one nearest it among the region's lowest, so the support stays
    // as wide as the cull made it and every region rests on its lowest points.
    let tolerance = 0.01 * linear_slop();
    let plane_point = |p: &SetPoint| {
        let d = sub(p.geometry.point, origin);
        vec2(dot(d, u), dot(d, v))
    };
    let region_of = |q: Vec2, anchors: &[Point2D]| {
        (0..anchors.len()).min_by(|&a, &b| distance_squared2(anchors[a].p, q).total_cmp(&distance_squared2(anchors[b].p, q))).unwrap()
    };
    let mut lowest = [f32::INFINITY; MAX_MANIFOLD_POINTS];
    for p in points.iter() {
        let r = region_of(plane_point(p), &projected[..count]);
        lowest[r] = lowest[r].min(p.geometry.separation);
    }
    let mut chosen = [(f32::INFINITY, usize::MAX); MAX_MANIFOLD_POINTS];
    for (i, p) in points.iter().enumerate() {
        let q = plane_point(p);
        let r = region_of(q, &projected[..count]);
        if projected[r].separation > lowest[r] + tolerance && p.geometry.separation <= lowest[r] + tolerance {
            let distance = distance_squared2(projected[r].p, q);
            if distance < chosen[r].0 {
                chosen[r] = (distance, i);
            }
        }
    }
    for (anchor, (_, index)) in projected[..count].iter_mut().zip(chosen) {
        if index != usize::MAX {
            anchor.original_index = index as i32;
        }
    }
    let mut kept = [SetPoint::default(); MAX_MANIFOLD_POINTS];
    for (dst, p) in kept.iter_mut().zip(&projected[..count]) {
        *dst = points[p.original_index as usize];
    }
    points.truncate(count);
    points.copy_from_slice(&kept[..count]);
}

const PATCH_NORMAL_DOT: f32 = 0.996;

// Return the normal in A's frame and the supplying side. A veto by B returns zero.
fn set_pair_direction(
    set_a: &BoxSetData,
    set_b: &BoxSetData,
    row: Row,
    bounds_a: AABB,
    b_to_a: Transform,
    wa: Vec3,
    wb: Vec3,
    na: Vec3,
) -> (Vec3, FeatureOwner) {
    let bounds_b = box_set_get(set_b, row.slot_b).unwrap().bounds;
    let incoming = neg(inv_rotate_vector(b_to_a.q, na));
    let masked_b = exterior_normal(set_b, row.slot_b, bounds_b, inv_transform_point(b_to_a, wb), incoming);
    let nb = neg(rotate_vector(b_to_a.q, masked_b));
    // Keep A's precise normal when the two would share a patch.
    // A larger turn must also be exterior at A's original witness.
    if dot(na, nb) > PATCH_NORMAL_DOT {
        (na, FeatureOwner::ShapeA)
    } else if masked_b != Vec3::ZERO && exterior_normal(set_a, row.slot_a, bounds_a, wa, nb) == nb {
        (nb, FeatureOwner::ShapeB)
    } else {
        (Vec3::ZERO, FeatureOwner::ShapeB)
    }
}

/// Replay an ordinary set/set row at its build pose for diagnostics, including after pose recycling.
/// Return its unfiltered normal, accepted row normal (both in world space), and the supplying side.
#[doc(hidden)]
pub fn debug_set_pair_row(
    world: &World,
    contact: &Contact,
    manifold: usize,
    point: usize,
) -> Option<(Vec3, Vec3, FeatureOwner)> {
    let cache = contact.set_contact.as_deref()?;
    let a = world.shapes.get(contact.shape_id_a as usize)?;
    let b = world.shapes.get(contact.shape_id_b as usize)?;
    let (ShapeGeometry::BoxSet(set_a), ShapeGeometry::BoxSet(set_b)) = (&a.geom, &b.geom) else { return None };
    let patch = contact.manifolds.get(manifold)?;
    let target = patch.points[..patch.point_count as usize].get(point)?;
    let slots = cache.point_children(a, b, manifold, point)?;
    if !cache.revisions_match(a, b) || target.feature_id >> 24 == u32::from(RECOVERY_OWNER) {
        return None;
    }
    let row = cache.rows[manifold * MAX_MANIFOLD_POINTS + point];
    let index = cache.pairs.binary_search_by_key(&slots, |p| p.children).ok()?;
    let mut sat = cache.pairs[index].cache.sat_cache;
    let bounds_a = box_set_get(set_a, row.slot_a)?.bounds;
    let bounds_b = box_set_get(set_b, row.slot_b)?.bounds;
    let mut hull_a = None;
    let mut hull_b = None;
    let hull_a = scratch_box(&mut hull_a);
    let hull_b = scratch_box(&mut hull_b);
    write_box(hull_a, bounds_a);
    write_box(hull_b, bounds_b);
    let b_to_a = contact.cached_relative_pose;
    let mut local = LocalManifold::default();
    collide_hulls(&mut local, MAX_MANIFOLD_POINTS as i32, hull_a, hull_b, b_to_a, &mut sat, world.speculative_distance);
    let rot = make_matrix_from_quat(contact.cached_rotation_a);
    let center = crate::body::get_body_sim_from_id(world, a.body_id).local_center;
    let center = rotate_vector(contact.cached_rotation_a, center);
    // Several child points can share a cluster's normal. Identify the reduced row with
    // its own feature and exact stored COM anchor before replaying its mask decision.
    let g = local.points.iter().find(|g| {
        make_feature_id(g.pair) == target.feature_id && sub(mul_mv(rot, g.point), center) == target.anchor_a
    })?;
    let old = local.normal;
    let wb = mul_add(g.point, 0.5 * g.separation, old);
    let wa = mul_sub(wb, g.separation, old);
    let na = exterior_normal(set_a, row.slot_a, bounds_a, wa, old);
    let (n, side) = set_pair_direction(set_a, set_b, row, bounds_a, b_to_a, wa, wb, na);
    (n != Vec3::ZERO).then(|| (mul_mv(rot, old), mul_mv(rot, n), side))
}

fn cluster_for<'a>(
    clusters: &'a mut Vec<Cluster>,
    count: &mut usize,
    normal: Vec3,
    plane: Plane,
    materials: [usize; 2],
) -> &'a mut Cluster {
    if let Some(i) = clusters[..*count].iter().position(|c| {
        c.patch.materials == materials
            && dot(c.normal, normal) > PATCH_NORMAL_DOT
            && dot(c.patch.plane.normal, plane.normal) > PATCH_NORMAL_DOT
            && (plane.offset - c.patch.plane.offset).abs() <= linear_slop()
    }) {
        return &mut clusters[i];
    }
    if *count == clusters.len() {
        clusters.push(Cluster::default());
    }
    let c = &mut clusters[*count];
    *count += 1;
    c.points.clear();
    c.normal = normal;
    c.patch = Patch { plane, materials, ..Default::default() };
    c
}

// Geometry is in the narrow phase's local frame. A mesh or height field is
// always side A, so its set's outward normal is the negative contact normal.
#[derive(Clone, Copy, Debug)]
struct DroppedPoint {
    point: SetPoint,
    other: Vec3, // in the recovering set's frame
    materials: [usize; 2],
    set_is_b: bool,
}

fn remember_drop(
    old: &mut Option<DroppedPoint>,
    point: SetPoint,
    other: Vec3,
    materials: [usize; 2],
    set_is_b: bool,
    set: &BoxSetData,
) {
    let separation = point.geometry.separation;
    if old.is_none_or(|prev| {
        if separation != prev.point.geometry.separation {
            return separation < prev.point.geometry.separation;
        }
        if separation >= -linear_slop() || prev.set_is_b != set_is_b || prev.point.row != point.row
            || prev.point.geometry.triangle_index != point.geometry.triangle_index
        {
            return false;
        }
        // A clipped face can have four equally deep points along the buried
        // normal. Prefer the one farther inside the set bounds: the leading
        // corner may already be out while the trailing corner is still inside.
        // This only breaks a tie. The six walks still find the actual exterior.
        let bounds = set.bounds().unwrap();
        let depth = |p| {
            let margin = min(sub(p, bounds.lower_bound), sub(bounds.upper_bound, p));
            margin.x.min(margin.y).min(margin.z)
        };
        depth(other) > depth(prev.other)
    }) {
        *old = Some(DroppedPoint { point, other, materials, set_is_b });
    }
}

fn cluster_faces(
    scratch: &mut SetCollideScratch,
    cache: &mut SetContact,
    set: &BoxSetData,
    a_to_b: Transform,
    corrected_rows: &mut usize,
    dropped_rows: &mut usize,
) -> usize {
    let mut count = 0;
    for face in &scratch.faces {
        let triangle = cache.triangles[face.triangle];
        if !face.accepted {
            let flags = triangle.geometry.flags;
            if flags & ALL_FLAT_EDGES == ALL_FLAT_EDGES {
                continue;
            }
            let shared = edges(face.slot, triangle.geometry)
                .into_iter()
                .zip([FLAT_EDGE1, FLAT_EDGE2, FLAT_EDGE3])
                .any(|(edge, flag)| flags & flag != 0 && scratch.edges.binary_search(&edge).is_ok());
            if shared {
                continue;
            }
        }
        let plane = triangle.plane;
        let materials = [triangle.material, face.material];
        let points = &scratch.points[face.points.clone()];
        if set.indexed_box_count() == 1 {
            cluster_for(&mut scratch.clusters, &mut count, face.normal, plane, materials)
                .points
                .extend_from_slice(points);
            continue;
        }
        let bounds = box_set_get(set, face.slot).unwrap().bounds;
        let mut kept = false;
        for mut p in points.iter().copied() {
            let g = p.geometry;
            let on_box = if face.midpoint { mul_add(g.point, 0.5 * g.separation, face.normal) } else { g.point };
            let outward = neg(face.normal);
            let n = exterior_normal(set, face.slot, bounds, on_box, outward);
            let normal = if n == outward { face.normal } else { neg(n) };
            let mut patch_plane = plane;
            if n != outward {
                if n == Vec3::ZERO || dot(outward, n).abs() < std::f32::consts::FRAC_1_SQRT_2 {
                    let other = mul_sub(on_box, g.separation, face.normal);
                    *dropped_rows += 1;
                    remember_drop(&mut scratch.deepest_drop, p, other, materials, true, set);
                    continue;
                }
                p.project_separation(face.normal, normal);
                *corrected_rows += 1;
                // Cluster by the corrected plane through the box witness in terrain coordinates.
                let axis = inv_rotate_vector(a_to_b.q, normal);
                patch_plane = Plane { normal: axis, offset: dot(axis, inv_transform_point(a_to_b, on_box)) };
            }
            cluster_for(&mut scratch.clusters, &mut count, normal, patch_plane, materials).points.push(p);
            kept = true;
        }
        if !kept {
            cache.pairs[face.pair_index].cache.sat_cache = SATCache::default();
        }
    }
    for c in &mut scratch.clusters[..count] {
        reduce_cluster(&mut c.points, &mut scratch.projected, rotate_vector(a_to_b.q, c.patch.plane.normal));
    }
    count
}

fn collide_convex_pairs(
    cache: &mut SetContact,
    scratch: &mut SetCollideScratch,
    set_a: &BoxSetData,
    material_counts: [usize; 2],
    geometry_b: &ShapeGeometry,
    b_to_a: Transform,
    speculative: f32,
    convex_bounds: Option<(AABB, AABB)>,
    sat_calls: &mut i32,
    corrected_rows: &mut usize,
    dropped_rows: &mut usize,
) -> (usize, bool) {
    if cache.pairs.is_empty() {
        return (0, false);
    }
    let [storage_a, storage_b] = &mut scratch.hulls;
    let hull = scratch_box(storage_a);
    let local = &mut scratch.local;
    let capacity = MAX_MANIFOLD_POINTS as i32;
    let mut count = 0;
    let mut missing_patch = false;
    for pair in &mut cache.pairs {
        let (slot_a, slot_b) = pair.children;
        let child = box_set_get(set_a, slot_a).unwrap();
        if let Some((current, reachable)) = convex_bounds {
            if !aabb_overlaps(current, child.bounds) {
                // This child only guards reuse. It needs no hull or GJK/SAT call.
                missing_patch |= aabb_overlaps(reachable, child.bounds);
                continue;
            }
        }
        write_box(hull, child.bounds);
        let (radius_b, material_b) = match geometry_b {
            ShapeGeometry::Sphere(sphere) => {
                collide_hull_and_sphere(
                    local,
                    capacity,
                    hull,
                    sphere,
                    b_to_a,
                    &mut pair.cache.simplex_cache,
                    speculative,
                );
                (sphere.radius, 0)
            }
            ShapeGeometry::Capsule(capsule) => {
                collide_hull_and_capsule(
                    local,
                    capacity,
                    hull,
                    capsule,
                    b_to_a,
                    &mut pair.cache.simplex_cache,
                    speculative,
                );
                (capsule.radius, 0)
            }
            ShapeGeometry::Hull(other) => {
                *sat_calls += 1;
                collide_hulls(local, capacity, hull, other, b_to_a, &mut pair.cache.sat_cache, speculative);
                (0.25 * other.inner_radius, 0)
            }
            ShapeGeometry::BoxSet(set_b) => {
                *sat_calls += 1;
                let child_b = box_set_get(set_b, slot_b).unwrap();
                let other = scratch_box(storage_b);
                write_box(other, child_b.bounds);
                collide_hulls(local, capacity, hull, other, b_to_a, &mut pair.cache.sat_cache, speculative);
                (0.25 * other.inner_radius, child_b.material)
            }
            _ => unreachable!("unregistered set/convex pair"),
        };
        if local.points.is_empty() {
            missing_patch |= convex_bounds.is_some();
            continue;
        }
        // Use the supporting box face in A's frame, including for an edge or
        // vertex contact. Support-face lookup breaks ties in box topology order.
        let face = crate::hull::find_hull_support_face(hull, local.normal);
        let plane = hull.planes[face as usize];
        let materials =
            [material_index(child.material, material_counts[0]), material_index(material_b, material_counts[1])];
        let row = Row { slot_a, serial_a: pair.serials.0, slot_b, serial_b: pair.serials.1 };
        let rolling_radius = max_float(0.25 * hull.inner_radius, radius_b);
        let old = local.normal;
        if set_a.indexed_box_count() == 1
            && !matches!(geometry_b, ShapeGeometry::BoxSet(b) if b.indexed_box_count() > 1)
        {
            let c = cluster_for(&mut scratch.clusters, &mut count, old, plane, materials);
            c.points.extend(local.points.iter().map(|g| SetPoint {
                geometry: LocalManifoldPoint { triangle_index: NULL_INDEX, ..*g },
                row,
                rolling_radius,
                corrected: false,
            }));
            continue;
        }
        let mut kept = false;
        for &g in &local.points {
            let mut point = SetPoint {
                geometry: LocalManifoldPoint { triangle_index: NULL_INDEX, ..g },
                row,
                rolling_radius,
                corrected: false,
            };
            // The deep sphere branch can have a coincident GJK simplex. Its
            // analytic surface witness is exact even when that simplex is interior.
            let wb = if let ShapeGeometry::Sphere(s) = geometry_b {
                mul_sub(transform_point(b_to_a, s.center), s.radius, old)
            } else {
                mul_add(g.point, 0.5 * g.separation, old)
            };
            let wa = mul_sub(wb, g.separation, old);
            let mut na = exterior_normal(set_a, slot_a, child.bounds, wa, old);
            if na != old && matches!(geometry_b, ShapeGeometry::Sphere(_) | ShapeGeometry::Capsule(_)) {
                // A face-interior GJK witness can have tangential roundoff.
                // Project the same core witness onto the exact box before
                // renormalizing what remains of a buried face normal.
                let core = mul_add(wb, radius_b, old);
                let closest = clamp(core, child.bounds.lower_bound, child.bounds.upper_bound);
                let axis = normalize(sub(core, closest));
                if axis != Vec3::ZERO {
                    na = exterior_normal(set_a, slot_a, child.bounds, closest, axis);
                }
            }
            if na == Vec3::ZERO
                || (na != old && matches!(geometry_b, ShapeGeometry::Hull(_))
                    && dot(old, na).abs() < std::f32::consts::FRAC_1_SQRT_2)
            {
                *dropped_rows += 1;
                remember_drop(&mut scratch.deepest_drop, point, wb, materials, false, set_a);
                continue;
            }
            let n = if let ShapeGeometry::BoxSet(set_b) = geometry_b {
                let (n, side) = set_pair_direction(set_a, set_b, row, child.bounds, b_to_a, wa, wb, na);
                // A turn past 45 degrees puts the original normal nearer a buried face than an exposed one.
                if n == Vec3::ZERO || (n != old && dot(old, n).abs() < std::f32::consts::FRAC_1_SQRT_2) {
                    *dropped_rows += 1;
                    if side == FeatureOwner::ShapeB {
                        let other_b = inv_transform_point(b_to_a, wa);
                        remember_drop(&mut scratch.deepest_drop, point, other_b, materials, true, set_b);
                    } else {
                        remember_drop(&mut scratch.deepest_drop, point, wb, materials, false, set_a);
                    }
                    continue;
                }
                n
            } else {
                na
            };
            let patch_plane = if n != old {
                point.project_separation(old, n);
                *corrected_rows += 1;
                let face = crate::hull::find_hull_support_face(hull, n);
                hull.planes[face as usize]
            } else {
                plane
            };
            cluster_for(&mut scratch.clusters, &mut count, n, patch_plane, materials).points.push(point);
            kept = true;
        }
        if !kept {
            // Motion along a buried face leaves its cached SAT separation
            // unchanged even as an exterior face becomes the least penetrating.
            pair.cache.sat_cache = SATCache::default();
            missing_patch |= convex_bounds.is_some();
        }
    }
    for c in &mut scratch.clusters[..count] {
        reduce_cluster(&mut c.points, &mut scratch.projected, c.patch.plane.normal);
    }
    (count, missing_patch)
}

fn index_old_points(scratch: &mut SetCollideScratch, old: &[Manifold], rows: &[Row]) {
    scratch.old_impulses.clear();
    scratch.old_points.clear();
    for (m, manifold) in old.iter().enumerate() {
        scratch.old_impulses.push(Some(PatchImpulses {
            friction: manifold.friction_impulse,
            rolling: manifold.rolling_impulse,
            twist: manifold.twist_impulse,
        }));
        for (p, point) in manifold.points[..manifold.point_count as usize].iter().enumerate() {
            let key = (rows[m * MAX_MANIFOLD_POINTS + p], point.triangle_index, point.feature_id);
            scratch.old_points.push(OldPoint {
                key,
                manifold: m,
                point: p,
                impulse: point.normal_impulse,
                normal: manifold.normal,
                claimed: false,
            });
        }
    }
    // The ordinals provide a stable tie-break even for coincident features.
    scratch.old_points.sort_unstable_by_key(|p| (p.key, p.manifold, p.point));
}

fn match_point(old: &mut [OldPoint], key: PointKey, normal: Vec3) -> Option<&mut OldPoint> {
    let start = old.partition_point(|p| p.key < key);
    old[start..].iter_mut().take_while(|p| p.key == key).find(|p| !p.claimed && dot(p.normal, normal) > 0.996)
}

fn move_patch_impulses(sources: &[usize], old: &mut [Option<PatchImpulses>], target: &mut Manifold) {
    let mut best = usize::MAX;
    let mut votes = 0;
    for &m in sources {
        if m == usize::MAX || old[m].is_none() {
            continue;
        }
        let count = sources.iter().filter(|&&source| source == m).count();
        if count > votes || (count == votes && m < best) {
            votes = count;
            best = m;
        }
    }
    // A split may share points from one old patch with two new patches.
    // Move its central impulses only once, in canonical new-patch order.
    if let Some(source) = old.get_mut(best).and_then(Option::take) {
        target.friction_impulse = source.friction;
        target.twist_impulse = source.twist;
        target.rolling_impulse = source.rolling;
    }
}

#[allow(clippy::too_many_arguments)]
fn write_manifolds(
    world: &World,
    contact: &mut Contact,
    cache: &mut SetContact,
    scratch: &mut SetCollideScratch,
    count: usize,
    a: &Shape,
    xf_a: WorldTransform,
    b: &Shape,
    xf_b: WorldTransform,
) {
    std::mem::swap(&mut cache.rows, &mut cache.prev_rows);
    index_old_points(scratch, &contact.manifolds, &cache.prev_rows);
    cache.resize_manifolds(&mut contact.manifolds, count);
    cache.rows.clear();
    let row_count = count * MAX_MANIFOLD_POINTS;
    cache.rows.resize(row_count, Row::default());
    // Both banks can now serve this many rows after the next swap.
    cache.prev_rows.reserve(row_count.saturating_sub(cache.prev_rows.len()));
    cache.patches.clear();

    // Convex routines return geometry in A, triangle routines in B.
    let in_a = matches!(a.geom, ShapeGeometry::BoxSet(_));
    let (frame, other) = if in_a { (xf_a, xf_b) } else { (xf_b, xf_a) };
    let offset = sub_pos(frame.p, other.p);
    let rot = make_matrix_from_quat(frame.q);
    let materials_a = get_shape_materials(a);
    let materials_b = get_shape_materials(b);
    let rest = if in_a { 0.0 } else { mesh_rest_offset() };
    let mut rolling = 0.0;
    let mut mix = PatchMaterialMix::default();

    for (m, cluster) in scratch.clusters[..count].iter().enumerate() {
        let mut patch = cluster.patch;
        let ma = materials_a[patch.materials[0]];
        let mb = materials_b[patch.materials[1]];
        mix.add(world, &mut patch, ma, mb, xf_a.q, xf_b.q, cluster.points.len());
        let resistance =
            if in_a { max_float(ma.rolling_resistance, mb.rolling_resistance) } else { mb.rolling_resistance };
        let manifold = &mut contact.manifolds[m];
        manifold.normal = mul_mv(rot, cluster.normal);
        manifold.point_count = cluster.points.len() as i32;
        let mut sources = [usize::MAX; MAX_MANIFOLD_POINTS];
        for (p, point) in cluster.points.iter().enumerate() {
            cache.rows[m * MAX_MANIFOLD_POINTS + p] = point.row;
            let target = &mut manifold.points[p];
            let geometry = point.geometry;
            let anchor = mul_mv(rot, geometry.point);
            if point.corrected {
                // Motion across the seam can change which normal components survive.
                contact.flags &= !crate::contact::RELATIVE_TRANSFORM_VALID;
            }
            if in_a {
                target.anchor_a = anchor;
                target.anchor_b = add(anchor, offset);
            } else {
                target.anchor_b = anchor;
                target.anchor_a = add(anchor, offset);
            }
            target.separation = if geometry.pair.owner1 == RECOVERY_OWNER {
                geometry.separation
            } else { geometry.separation - rest };
            target.triangle_index = geometry.triangle_index;
            target.feature_id = make_feature_id(geometry.pair);
            if let Some(old) =
                match_point(&mut scratch.old_points, (point.row, target.triangle_index, target.feature_id), manifold.normal)
            {
                old.claimed = true;
                sources[p] = old.manifold;
                target.persisted = true;
                target.normal_impulse = old.impulse;
            }
            let scaled = resistance * point.rolling_radius;
            patch.rolling_resistance += scaled;
            rolling += scaled;
        }
        move_patch_impulses(&sources, &mut scratch.old_impulses, manifold);
        let n = cluster.points.len() as f32;
        // Boxes sharing a patch can have different inner radii. Average over
        // the same reduced points that carry this manifold's normal load.
        patch.rolling_resistance /= n;
        cache.patches.push(patch);
    }
    if mix.points > 0 {
        contact.rolling_resistance = rolling * (1.0 / mix.points as f32);
    }
    mix.apply(contact);
}

// Keep the point-weighted contact coefficients and the individual patches in
// sync, both at manifold creation and when either material changes at rest.
#[derive(Default)]
struct PatchMaterialMix {
    points: usize,
    friction: f32,
    restitution: f32,
    yield_force: f32,
    tangent: Vec3,
}

impl PatchMaterialMix {
    fn add(
        &mut self, world: &World, patch: &mut Patch, a: SurfaceMaterial, b: SurfaceMaterial,
        rotation_a: Quat, rotation_b: Quat, points: usize,
    ) {
        patch.friction = mix_friction(world, a.friction, a.user_material_id, b.friction, b.user_material_id);
        patch.restitution = mix_restitution(world, a.restitution, a.user_material_id, b.restitution, b.user_material_id);
        patch.yield_force = mix_yield_force(a.yield_force, b.yield_force);
        patch.tangent_velocity =
            sub(rotate_vector(rotation_a, a.tangent_velocity), rotate_vector(rotation_b, b.tangent_velocity));
        let n = points as f32;
        self.friction += patch.friction * n;
        self.restitution += patch.restitution * n;
        self.yield_force = mix_yield_force(self.yield_force, patch.yield_force);
        self.tangent = mul_add(self.tangent, n, patch.tangent_velocity);
        self.points += points;
    }

    fn apply(&self, contact: &mut Contact) {
        if self.points > 0 {
            let inv = 1.0 / self.points as f32;
            contact.friction = self.friction * inv;
            contact.restitution = self.restitution * inv;
            contact.yield_force = self.yield_force;
            contact.tangent_velocity = mul_sv(inv, self.tangent);
        }
    }
}

pub(crate) fn remix_recycled_patches(world: &World, contact: &mut Contact, xf_a: WorldTransform, xf_b: WorldTransform) {
    let a = get_shape_materials(&world.shapes[contact.shape_id_a as usize]);
    let b = get_shape_materials(&world.shapes[contact.shape_id_b as usize]);
    let patches = &mut contact.set_contact.as_mut().unwrap().patches;
    let mut mix = PatchMaterialMix::default();
    for (patch, manifold) in patches.iter_mut().zip(&contact.manifolds) {
        let [i, j] = patch.materials;
        mix.add(world, patch, a[i], b[j], xf_a.q, xf_b.q, manifold.point_count as usize);
    }
    mix.apply(contact);
}

fn child_inner_radius(set: &BoxSetData, slot: i32) -> f32 {
    let h = aabb_extents(box_set_get(set, slot).unwrap().bounds);
    h.x.min(h.y).min(h.z)
}

fn recover_embedded(
    scratch: &mut SetCollideScratch,
    deepest: DroppedPoint,
    a: &Shape,
    b: &Shape,
    b_to_a: Transform,
) -> usize {
    let mut p = deepest.point;
    let (shape, slot) = if deepest.set_is_b { (b, p.row.slot_b) } else { (a, p.row.slot_a) };
    let ShapeGeometry::BoxSet(set) = &shape.geom else { unreachable!() };
    let exit = exterior_exit(set, slot, deepest.other);
    let mut materials = deepest.materials;
    materials[usize::from(deepest.set_is_b)] =
        material_index(box_set_get(set, exit.slot).unwrap().material, get_shape_materials(shape).len());
    let serial = set.serial(exit.slot).unwrap();
    if deepest.set_is_b {
        p.row.slot_b = exit.slot;
        p.row.serial_b = serial;
    } else {
        p.row.slot_a = exit.slot;
        p.row.serial_a = serial;
    }
    p.geometry.pair =
        crate::types::FeaturePair { owner1: RECOVERY_OWNER, index2: exit.direction as u8, ..Default::default() };
    p.geometry.separation = -exit.distance;
    let mut normal = exit.normal;
    let mut other = deepest.other;
    let mut face = exit.point;
    if let ShapeGeometry::BoxSet(set_a) = &a.geom {
        let radius_b = match &b.geom {
            ShapeGeometry::Sphere(s) => s.radius,
            ShapeGeometry::Capsule(c) => c.radius,
            ShapeGeometry::Hull(h) => 0.25 * h.inner_radius,
            ShapeGeometry::BoxSet(set_b) => 0.25 * child_inner_radius(set_b, p.row.slot_b),
            _ => unreachable!("unregistered set/convex recovery"),
        };
        p.rolling_radius = (0.25 * child_inner_radius(set_a, p.row.slot_a)).max(radius_b);
        if deepest.set_is_b {
            normal = rotate_vector(b_to_a.q, normal);
            other = transform_point(b_to_a, other);
            face = transform_point(b_to_a, face);
        }
    } else {
        p.rolling_radius = child_inner_radius(set, exit.slot);
    }
    if deepest.set_is_b {
        normal = neg(normal);
    }
    // The recovery force is applied at the one interior witness.
    p.geometry.point = other;
    p.corrected = false;
    let plane = Plane { normal, offset: dot(normal, face) };
    let mut count = 0;
    cluster_for(&mut scratch.clusters, &mut count, normal, plane, materials).points.push(p);
    exit.steps
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_set_manifolds(
    world: &World,
    task: &mut TaskContext,
    contact: &mut Contact,
    a: &Shape,
    xf_a: WorldTransform,
    b: &Shape,
    xf_b: WorldTransform,
    is_fast: bool,
) -> bool {
    let mut cache = contact.set_contact.take().unwrap();
    let scratch = task.set_scratch.get_or_insert_with(Default::default);
    let b_to_a = inv_mul_world_transforms(xf_a, xf_b);
    scratch.deepest_drop = None;
    let mut count = match (&a.geom, &b.geom) {
        (ShapeGeometry::Mesh(_) | ShapeGeometry::HeightField(_), ShapeGeometry::BoxSet(set)) => {
            let a_to_b = inv_mul_world_transforms(xf_b, xf_a);
            cache.refresh_triangles(a, set, b_to_a);
            task.set_mesh_node_visits += triangle_child_pairs(&mut cache, scratch, set, a_to_b);
            let hits = collide_triangle_pairs(&mut cache, scratch, set, get_shape_materials(b).len(), is_fast);
            task.sat_call_count += cache.pairs.len() as i32;
            task.sat_cache_hit_count += hits;
            task.set_triangle_visits += cache.triangles.len();
            cluster_faces(scratch, &mut cache, set, a_to_b, &mut task.set_corrected_rows, &mut task.set_dropped_rows)
        }
        (ShapeGeometry::BoxSet(set_a), geometry_b) => {
            let mut convex_bounds = None;
            task.set_box_visits += if let ShapeGeometry::BoxSet(set_b) = geometry_b {
                let a_to_b = inv_mul_world_transforms(xf_b, xf_a);
                set_child_pairs(&mut cache, &mut scratch.descent, set_a, set_b, a_to_b, b_to_a, world.speculative_distance)
            } else {
                let bounds = match geometry_b {
                    ShapeGeometry::Hull(hull) => transform_box_bounds(hull.aabb, b_to_a),
                    ShapeGeometry::Sphere(s) => crate::sphere::compute_sphere_aabb(s, b_to_a),
                    ShapeGeometry::Capsule(c) => crate::capsule::compute_capsule_aabb(c, b_to_a),
                    _ => unreachable!("unregistered set/convex pair"),
                };
                let bounds = aabb_inflate(bounds, world.speculative_distance);
                if set_a.indexed_box_count() > 1 && world.contact_recycle_distance > 0.0
                    && contact.flags & crate::contact::CONTACT_RECYCLE_FLAG != 0
                {
                    // A floor patch can already be touching while a second
                    // child is about to present a wall. Check every child the
                    // convex shape can reach during the parent's pose reuse.
                    let reachable = aabb_inflate(bounds, world.contact_recycle_distance);
                    convex_bounds = Some((bounds, reachable));
                    convex_child_pairs(&mut cache, set_a, reachable)
                } else {
                    convex_child_pairs(&mut cache, set_a, bounds)
                }
            };
            let (count, missing_patch) = collide_convex_pairs(
                &mut cache,
                scratch,
                set_a,
                [get_shape_materials(a).len(), get_shape_materials(b).len()],
                geometry_b,
                b_to_a,
                world.speculative_distance,
                convex_bounds,
                &mut task.sat_call_count,
                &mut task.set_corrected_rows,
                &mut task.set_dropped_rows,
            );
            if count > 0 && missing_patch {
                contact.flags &= !crate::contact::RELATIVE_TRANSFORM_VALID;
            }
            count
        }
        _ => unreachable!("unregistered set contact"),
    };
    if let Some(deepest) = scratch.deepest_drop {
        // Even if another patch survived unchanged, motion along a seam
        // can expose a discarded witness within the pose-recycling distance.
        contact.flags &= !crate::contact::RELATIVE_TRANSFORM_VALID;
        if count == 0 && deepest.point.geometry.separation < -linear_slop() {
            task.set_walk_steps += recover_embedded(scratch, deepest, a, b, b_to_a);
            task.set_recovery_contacts += 1;
            count = 1;
        }
    }
    task.set_child_pairs += cache.pairs.len();
    task.set_manifolds += count;
    write_manifolds(world, contact, &mut cache, scratch, count, a, xf_a, b, xf_b);
    contact.set_contact = Some(cache);
    count > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::box_set::{create_box_set, SetBox};
    use crate::contact::{has_yield_force, yield_impulse_share};

    #[test]
    fn yield_budget_handles_rigid_and_unrepresentable_shares() {
        let h = 1.0 / 256.0;
        for force in [0.0, -0.0, -5.0, f32::NAN, f32::INFINITY] {
            assert!(!has_yield_force(mix_yield_force(force, force), h), "force {force} must be rigid");
        }
        let tiny = f32::from_bits(1);
        assert!(has_yield_force(tiny, h), "a finite yielding surface still participates");
        assert_eq!(yield_impulse_share(tiny, h, 4).to_bits(), 0, "no representable impulse fits");
        assert_eq!(mix_yield_force(f32::NAN, 2000.0), 2000.0, "a NaN side is rigid and defers to the other");
        assert_eq!(mix_yield_force(-0.0, 2000.0), 2000.0, "a -0 side is rigid and defers to the other");
        assert_eq!(mix_yield_force(3000.0, 2000.0), 2000.0, "two yielding sides give at the smaller force");
        assert_eq!(yield_impulse_share(24_000.0, h, 1), 24_000.0 * h, "a finite force gives force times the substep");
    }

    fn seeded_fractions(mut seed: u32) -> impl FnMut() -> f32 {
        move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        }
    }

    #[test]
    fn set_pair_descent_emits_exactly_the_per_box_candidates() {
        // The candidate rule: an outer box meets the inner set's padded bounds,
        // and an inner box meets that outer box's padded bounds in the inner frame.
        let mut next = seeded_fractions(0x9e37_79b9);
        let boxed = |lo: Vec3, hi: Vec3| SetBox { bounds: AABB { lower_bound: lo, upper_bound: hi }, material: 0, user_data: 0 };
        let mut crown = vec![boxed(vec3(-0.1, -1.0, -0.1), vec3(0.1, 0.4, 0.1))];
        let mut block = Vec::new();
        for i in 0..8 {
            for k in 0..8 {
                let (x, z) = (-0.8 + 0.2 * i as f32, -0.8 + 0.2 * k as f32);
                if next() < 0.8 {
                    crown.push(boxed(vec3(x, 0.4, z), vec3(x + 0.2, 0.5 + 0.2 * next(), z + 0.2)));
                }
                if i < 6 && k < 4 && next() < 0.7 {
                    block.push(boxed(vec3(x, -0.15, z), vec3(x + 0.2, 0.05, z + 0.2)));
                }
            }
        }
        let (a, b) = (create_box_set(&crown), create_box_set(&block));
        let mut stack = Vec::new();
        let mut emitted = 0;
        for round in 0..64 {
            let axis = normalize(vec3(next() - 0.5, next() - 0.5, next() - 0.5));
            let q = make_quat_from_axis_angle(axis, 6.3 * next());
            let b_to_a = Transform { p: vec3(2.0 * next() - 1.0, 1.2 * next() - 0.6, 2.0 * next() - 1.0), q };
            let a_to_b = invert_transform(b_to_a);
            let margin = if round % 2 == 0 { 0.04 } else { 0.0 };
            for (x, y, x_to_y, y_to_x) in [(&a, &b, a_to_b, b_to_a), (&b, &a, b_to_a, a_to_b)] {
                let mut cache = SetContact::default();
                set_child_pairs(&mut cache, &mut stack, x, y, x_to_y, y_to_x, margin);
                let descended: Vec<_> = cache.pairs.iter().map(|p| (p.children, p.serials)).collect();
                let outer_is_x = x.indexed_box_count() <= y.indexed_box_count();
                let (outer, inner, to_outer, to_inner) =
                    if outer_is_x { (x, y, y_to_x, x_to_y) } else { (y, x, x_to_y, y_to_x) };
                let query = aabb_inflate(transform_box_bounds(inner.bounds().unwrap(), to_outer), margin);
                let mut expected = Vec::new();
                for (o, ob) in outer.boxes().filter(|(_, ob)| aabb_overlaps(ob.bounds, query)) {
                    let seen = aabb_inflate(transform_box_bounds(ob.bounds, to_inner), margin);
                    for (i, _) in inner.boxes().filter(|(_, ib)| aabb_overlaps(ib.bounds, seen)) {
                        let (so, si) = (outer.serial(o).unwrap(), inner.serial(i).unwrap());
                        expected.push(if outer_is_x { ((o, i), (so, si)) } else { ((i, o), (si, so)) });
                    }
                }
                expected.sort_unstable();
                assert_eq!(descended, expected, "round {round}: the descent must emit exactly the per-box candidates");
                emitted += expected.len();
            }
        }
        assert!(emitted > 200, "the poses must produce real candidates, got {emitted}");
    }

    #[test]
    fn triangle_pair_descent_matches_brute_force_and_preserves_fresh_contacts() {
        let mut boxes = Vec::new();
        for x in 0..8 {
            for z in 0..8 {
                let center = vec3((x as f32 - 3.5) * 0.14, 0.0, (z as f32 - 3.5) * 0.14);
                let half = vec3(0.04, 0.02, 0.04);
                let bounds = AABB { lower_bound: sub(center, half), upper_bound: add(center, half) };
                boxes.push(SetBox { bounds, material: 0, user_data: 0 });
            }
        }
        let set = create_box_set(&boxes);
        // Boxes with x == z straddle the shared diagonal of the two triangles.
        let surface = Shape {
            geom: ShapeGeometry::Mesh(crate::types::Mesh {
                data: crate::mesh::create_grid_mesh(1, 1, 0.8, 0, true),
                scale: Vec3::ONE,
            }),
            ..Default::default()
        };
        let mut cache = SetContact::default();
        cache.refresh_triangles(&surface, &set, Transform::IDENTITY);
        assert_eq!(cache.triangles.len(), 2, "the fixture must exercise both sides of the shared edge");

        let mut scratch = SetCollideScratch::default();
        let mut next = seeded_fractions(0x9e37_79b9);
        let mut removed_any = false;
        let mut kept_contact = false;
        let limit = speculative_distance() + linear_slop();
        for tilt in (0..=30).step_by(5) {
            for height_cm in (-5..=10).step_by(3) {
                let (sin, cos) = (2.0 * PI * next()).sin_cos();
                let q = make_quat_from_axis_angle(vec3(cos, 0.0, sin), (tilt as f32).to_radians());
                let to_set = invert_transform(Transform { p: vec3(0.0, height_cm as f32 * 0.01, 0.0), q });
                triangle_child_pairs(&mut cache, &mut scratch, &set, to_set);
                let hull = scratch_box(&mut scratch.hulls[1]);
                let mut expected = Vec::new();
                for (i, triangle) in cache.triangles.iter().enumerate() {
                    let vertices = triangle.geometry.vertices.map(|v| transform_point(to_set, v));
                    let [a, b, c] = vertices;
                    let query = make_aabb(&vertices, speculative_distance());
                    let plane = make_plane_from_points(a, b, c);
                    // Every fixture box is indexed with DEFAULT_MASK_BITS.
                    for (slot, child) in set.boxes().filter(|(_, child)| aabb_overlaps(child.bounds, query)) {
                        write_box(hull, child.bounds);
                        let removed = hull.points.iter().all(|&p| plane_separation(plane, p) > limit);
                        crate::triangle_manifold::collide_hull_and_triangle(
                            &mut scratch.local, i32::MAX, hull, a, b, c, triangle.geometry.flags, &mut SATCache::default(),
                        );
                        if removed {
                            removed_any = true;
                            assert!(
                                scratch.local.points.is_empty(),
                                "culled triangle {} and box {slot} must have no fresh points at {tilt} degrees and {height_cm} cm",
                                triangle.index,
                            );
                        } else {
                            expected.push(((triangle.index, slot), (0, set.serial(slot).unwrap()), i));
                            kept_contact |= !scratch.local.points.is_empty();
                        }
                    }
                }
                expected.sort_unstable();
                let actual: Vec<_> = cache.pairs.iter().map(|p| (p.children, p.serials, p.triangle)).collect();
                assert_eq!(actual, expected, "the descent must match the per-box scan at {tilt} degrees and {height_cm} cm");
            }
        }
        assert!(removed_any, "the plane must reject at least one overlapping AABB candidate");
        assert!(kept_contact, "at least one surviving pair must produce fresh contact points");
    }
}
