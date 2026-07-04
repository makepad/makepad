// Port of box3d/src/mesh.c — triangle mesh builder (binned SAH / median split BVH),
// vertex welding, edge identification, and mesh queries.
// Dirk Gregorius contributed portions of the original C code.
//
// Deviations (see PORTING.md):
// - b3MeshData trailing arrays + byte offsets -> MeshData Vec fields; creation
//   returns Arc<MeshData>. destroy_mesh is not ported (Arc drop).
// - create_mesh returns Option<Arc<MeshData>> (C returns NULL on invalid input,
//   insane bounds, or BVH-height overflow in triangle sorting).
// - degenerate triangle reporting: C fills a caller buffer up to capacity-1
//   entries; the port pushes all geometric degenerates into an unbounded Vec.
// - The C mesh->materialCount (distinct material count, min 1) has no field in
//   MeshData; use get_mesh_material_count() which recomputes the same value.
// - The C code hashes the raw allocation; the port hashes a canonical
//   serialization (bounds, surface_area, tree_height, degenerate_count, nodes,
//   vertices, triangles, materials, flags — all little-endian). Values differ
//   from C but are self-consistent, which is all dedup needs.
// - The vertex-weld and edge maps are verstable u64->int maps used only for
//   keyed lookup (never iterated), so std HashMap is deterministic here.
// - b3CreateWaveMesh uses libm sinf in C; the port uses f32::sin. Test
//   scaffolding only.
// - In the B3_SIMD_NONE build b3V32 is {x,y,z} — identical to Vec3 — so the
//   b3*V wrappers become plain Vec3 math. The four shared helpers implemented
//   in simd.h/simd.c (test_bounds_overlap, test_bounds_ray_overlap,
//   test_bounds_triangle_overlap, intersect_ray_triangle) are referenced from
//   crate::simd.

use std::collections::HashMap;
use std::sync::Arc;

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::{linear_slop, MAX_SHAPE_CAST_POINTS};
use crate::core::{hash, non_zero_hash, HASH_INIT, NULL_INDEX};
use crate::math_functions::{
    aabb_area, aabb_center, aabb_contains, aabb_extents, aabb_transform, aabb_union, abs_float,
    add, cos, cross, dot, is_sane_aabb, length, length_squared, make_aabb, max, max_int, min,
    min_int, mul, mul_sv, neg, normalize, sin, sub, vec3, Plane, Quat, Transform, Vec3, AABB,
};
use crate::math_internal::{
    aabb_add_point, get_by_index, major_axis, signed_volume, Triangle, BOUNDS3_EMPTY, TWO_PI,
};
use crate::types::{
    Capsule, CastOutput, DistanceInput, Mesh, MeshData, MeshDef, MeshNode, MeshTriangle,
    PlaneResult, RayCastInput, ShapeCastInput, ShapeCastPairInput, ShapeProxy, SimplexCache,
    CONCAVE_EDGE1, CONCAVE_EDGE2, CONCAVE_EDGE3, INVERSE_CONCAVE_EDGE1, INVERSE_CONCAVE_EDGE2,
    INVERSE_CONCAVE_EDGE3,
};

const BIN_COUNT: usize = 8;
const DESIRED_TRIANGLES_PER_LEAF: i32 = 4;
const MAXIMUM_TRIANGLES_PER_LEAF: i32 = 8;
const MESH_STACK_SIZE: usize = 256;

// Component-wise divide (C b3DivV in the scalar path).
#[inline(always)]
fn div_v(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x / b.x, a.y / b.y, a.z / b.z)
}

// Vec3 adapters over the crate::simd helpers (C loads b3V32 via b3LoadV at the
// call sites; the loads are folded in here).
#[inline(always)]
fn test_bounds_overlap(node_min1: Vec3, node_max1: Vec3, node_min2: Vec3, node_max2: Vec3) -> bool {
    crate::simd::test_bounds_overlap(
        crate::simd::load_vec3(node_min1),
        crate::simd::load_vec3(node_max1),
        crate::simd::load_vec3(node_min2),
        crate::simd::load_vec3(node_max2),
    )
}

#[inline(always)]
fn test_bounds_ray_overlap(node_min: Vec3, node_max: Vec3, ray_start: Vec3, ray_delta: Vec3) -> bool {
    crate::simd::test_bounds_ray_overlap(
        crate::simd::load_vec3(node_min),
        crate::simd::load_vec3(node_max),
        crate::simd::load_vec3(ray_start),
        crate::simd::load_vec3(ray_delta),
    )
}

#[inline(always)]
fn test_bounds_triangle_overlap(node_center: Vec3, node_extent: Vec3, vertex1: Vec3, vertex2: Vec3, vertex3: Vec3) -> bool {
    crate::simd::test_bounds_triangle_overlap(
        crate::simd::load_vec3(node_center),
        crate::simd::load_vec3(node_extent),
        crate::simd::load_vec3(vertex1),
        crate::simd::load_vec3(vertex2),
        crate::simd::load_vec3(vertex3),
    )
}

#[inline(always)]
fn intersect_ray_triangle(ray_start: Vec3, ray_delta: Vec3, vertex1: Vec3, vertex2: Vec3, vertex3: Vec3) -> f32 {
    crate::simd::intersect_ray_triangle(
        crate::simd::load_vec3(ray_start),
        crate::simd::load_vec3(ray_delta),
        crate::simd::load_vec3(vertex1),
        crate::simd::load_vec3(vertex2),
        crate::simd::load_vec3(vertex3),
    )
}

// The left child follows its parent.
#[inline(always)]
fn left_child(node_index: i32) -> i32 {
    node_index + 1
}

// We store the offset of the right child relative to its parent.
#[inline(always)]
fn right_child(node_index: i32, node: &MeshNode) -> i32 {
    b3_assert!(!node.is_leaf());
    node_index + node.child_offset() as i32
}

fn get_node_height(nodes: &[MeshNode], node_index: i32) -> i32 {
    let node = &nodes[node_index as usize];
    if node.is_leaf() {
        return 0;
    }

    let left_height = get_node_height(nodes, left_child(node_index));
    let right_height = get_node_height(nodes, right_child(node_index, node));

    1 + max_int(left_height, right_height)
}

/// Get the height of the mesh BVH.
pub fn get_height(mesh: &MeshData) -> i32 {
    if mesh.nodes.is_empty() {
        return 0;
    }

    get_node_height(&mesh.nodes, 0)
}

fn is_degenerate_triangle(v1: Vec3, v2: Vec3, v3: Vec3, min_area: f32) -> bool {
    let normal = cross(sub(v2, v1), sub(v3, v1));
    let length_sq = length_squared(normal);
    length_sq < min_area * min_area
}

fn is_non_degenerate(mesh: &MeshData, min_area: f32) -> bool {
    let vertex_count = mesh.vertex_count();

    // Check triangles
    for triangle in &mesh.triangles {
        // Index range
        if triangle.index1 >= vertex_count {
            return false;
        }

        if triangle.index2 >= vertex_count {
            return false;
        }

        if triangle.index3 >= vertex_count {
            return false;
        }

        // Degenerate topology
        if triangle.index1 == triangle.index2 {
            return false;
        }
        if triangle.index1 == triangle.index3 {
            return false;
        }
        if triangle.index2 == triangle.index3 {
            return false;
        }

        // Degenerate geometry
        let vertex1 = mesh.vertices[triangle.index1 as usize];
        let vertex2 = mesh.vertices[triangle.index2 as usize];
        let vertex3 = mesh.vertices[triangle.index3 as usize];
        if is_degenerate_triangle(vertex1, vertex2, vertex3, min_area) {
            return false;
        }
    }

    true
}

#[inline]
fn get_node_aabb(node: &MeshNode) -> AABB {
    AABB { lower_bound: node.lower_bound, upper_bound: node.upper_bound }
}

fn is_consistent(mesh: &MeshData) -> bool {
    if mesh.nodes.is_empty() {
        return false;
    }

    // Check nodes (the C version uses a fixed stack of 64; a Vec is safe for validation)
    let mut stack: Vec<i32> = Vec::with_capacity(64);
    stack.push(0);

    while let Some(node_index) = stack.pop() {
        let node = &mesh.nodes[node_index as usize];
        let node_bounds = get_node_aabb(node);

        if !node.is_leaf() {
            let child1 = left_child(node_index);
            let bounds1 = get_node_aabb(&mesh.nodes[child1 as usize]);
            let child2 = right_child(node_index, node);
            let bounds2 = get_node_aabb(&mesh.nodes[child2 as usize]);

            if !aabb_contains(node_bounds, bounds1) {
                return false;
            }

            if !aabb_contains(node_bounds, bounds2) {
                return false;
            }

            stack.push(child2);
            stack.push(child1);
        } else {
            let mut triangle_bounds = BOUNDS3_EMPTY;
            for index in 0..node.triangle_count() {
                let triangle_index = node.triangle_offset as i32 + index as i32;
                b3_assert!(0 <= triangle_index && triangle_index < mesh.triangle_count());

                let triangle = mesh.triangles[triangle_index as usize];

                let mut vertex_bounds = BOUNDS3_EMPTY;
                vertex_bounds = aabb_add_point(vertex_bounds, mesh.vertices[triangle.index1 as usize]);
                vertex_bounds = aabb_add_point(vertex_bounds, mesh.vertices[triangle.index2 as usize]);
                vertex_bounds = aabb_add_point(vertex_bounds, mesh.vertices[triangle.index3 as usize]);

                triangle_bounds = aabb_union(triangle_bounds, vertex_bounds);
            }

            if !aabb_contains(node_bounds, triangle_bounds) {
                return false;
            }
        }
    }

    true
}

/// The C version checks version/byteCount of the raw allocation (not applicable
/// to the port) and runs the consistency check only in validation builds.
pub fn is_valid_mesh(mesh_data: &MeshData) -> bool {
    if mesh_data.nodes.is_empty() || mesh_data.triangles.is_empty() {
        return false;
    }

    #[cfg(debug_assertions)]
    {
        is_consistent(mesh_data)
    }
    #[cfg(not(debug_assertions))]
    {
        true
    }
}

// ---------------------------------------------------------------------------
// Vertex welding
// ---------------------------------------------------------------------------

// Node for a vertex linked list
#[derive(Clone, Copy)]
struct VertexNode {
    vertex_index: i32,
    next_node_index: i32,
}

struct SpatialHash<'a> {
    nodes: Vec<VertexNode>,
    vertices: &'a [Vec3],
    vertex_map: HashMap<u64, i32>,
    cell_size: f32,
    tolerance: f32,
}

// Compute hash for a grid cell (this is the key in the map).
// Note: the C code sign-extends the int32 coordinates to uint64.
#[inline]
fn spatial_cell_key(x: i32, y: i32, z: i32) -> u64 {
    let mut key: u64 = 0;
    key ^= (x as i64 as u64)
        .wrapping_add(0x9e3779b9)
        .wrapping_add(key << 6)
        .wrapping_add(key >> 2);
    key ^= (y as i64 as u64)
        .wrapping_add(0x9e3779b9)
        .wrapping_add(key << 6)
        .wrapping_add(key >> 2);
    key ^= (z as i64 as u64)
        .wrapping_add(0x9e3779b9)
        .wrapping_add(key << 6)
        .wrapping_add(key >> 2);
    key
}

impl<'a> SpatialHash<'a> {
    fn create(vertices: &'a [Vec3], tolerance: f32) -> SpatialHash<'a> {
        let h = SpatialHash {
            nodes: Vec::with_capacity(vertices.len()),
            vertices,
            vertex_map: HashMap::with_capacity(vertices.len()),
            cell_size: 2.0 * tolerance,
            tolerance,
        };

        b3_assert!(h.cell_size > 0.0);
        h
    }

    // Welding works by bucketing nearby vertices into identical keys in a hash table.
    // Bucketing is done manually with an array.
    fn find_duplicate(&mut self, current_index: i32) -> i32 {
        b3_assert!((current_index as usize) < self.vertices.len());
        let vertex = self.vertices[current_index as usize];
        let cell_size = self.cell_size;
        let tolerance = self.tolerance;

        // Get the grid coordinates for the current vertex
        let base_x = (vertex.x / cell_size).floor() as i32;
        let base_y = (vertex.y / cell_size).floor() as i32;
        let base_z = (vertex.z / cell_size).floor() as i32;

        // Check the current cell and all 26 neighboring cells (3x3x3 - 1)
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let x = base_x + dx;
                    let y = base_y + dy;
                    let z = base_z + dz;

                    let key = spatial_cell_key(x, y, z);

                    if let Some(&head) = self.vertex_map.get(&key) {
                        // Check all vertices in this key
                        let mut node_index = head;

                        while node_index != NULL_INDEX {
                            let node = self.nodes[node_index as usize];

                            let existing_index = node.vertex_index;
                            b3_assert!(existing_index < current_index);
                            b3_assert!((existing_index as usize) < self.vertices.len());

                            let other = self.vertices[existing_index as usize];

                            // IsEqual inlined: check if vertices are within tolerance
                            if abs_float(vertex.x - other.x) <= tolerance
                                && abs_float(vertex.y - other.y) <= tolerance
                                && abs_float(vertex.z - other.z) <= tolerance
                            {
                                // Found duplicate
                                return existing_index;
                            }

                            node_index = node.next_node_index;
                        }
                    }
                }
            }
        }

        // No duplicate found, add to hash table
        let current_key = spatial_cell_key(base_x, base_y, base_z);

        if let Some(head) = self.vertex_map.get_mut(&current_key) {
            let node = VertexNode { vertex_index: current_index, next_node_index: *head };

            *head = self.nodes.len() as i32;
            self.nodes.push(node);
        } else {
            let node = VertexNode { vertex_index: current_index, next_node_index: NULL_INDEX };

            self.vertex_map.insert(current_key, self.nodes.len() as i32);
            self.nodes.push(node);
        }

        // Not welded
        NULL_INDEX
    }
}

fn weld_vertices(
    src_vertices: &[Vec3],
    src_indices: &[i32],
    dst_vertices: &mut [Vec3],
    dst_indices: &mut [i32],
    tolerance: f32,
) -> i32 {
    let vertex_count = src_vertices.len() as i32;
    let mut unique_count = 0;

    // Create spatial hash and find duplicates
    let mut spatial_hash = SpatialHash::create(src_vertices, tolerance);
    let mut vertex_mapping = vec![0i32; vertex_count as usize];

    for i in 0..vertex_count {
        let duplicate_index = spatial_hash.find_duplicate(i);

        if duplicate_index == NULL_INDEX {
            // New unique vertex
            vertex_mapping[i as usize] = unique_count;
            dst_vertices[unique_count as usize] = src_vertices[i as usize];
            unique_count += 1;
        } else {
            // Found duplicate, map to existing vertex
            vertex_mapping[i as usize] = vertex_mapping[duplicate_index as usize];
        }
    }

    // Update indices to reference the new vertex array
    let index_count = src_indices.len();
    for i in 0..index_count {
        let src_index = src_indices[i];
        b3_assert!(src_index < vertex_count);
        dst_indices[i] = vertex_mapping[src_index as usize];
    }

    unique_count
}

// ---------------------------------------------------------------------------
// BVH build
// ---------------------------------------------------------------------------

#[inline]
fn store_leaf(node: &mut MeshNode, aabb: &AABB, triangle_count: i32, triangle_offset: i32) {
    node.data = MeshNode::pack_leaf(triangle_count as u32);
    node.triangle_offset = triangle_offset as u32;
    node.lower_bound = aabb.lower_bound;
    node.upper_bound = aabb.upper_bound;
}

#[derive(Clone, Copy)]
struct Primitive {
    aabb: AABB,
    center: Vec3,
    triangle_index: i32,
}

#[derive(Clone, Copy)]
struct Bucket {
    count: i32,
    bounds: AABB,
}

#[derive(Clone, Copy)]
struct Split {
    left_bounds: AABB,
    right_bounds: AABB,
    axis: i32,
    index: i32,
}

fn split_binned_sah(primitives: &mut [Primitive]) -> Split {
    let count = primitives.len() as i32;

    let mut split = Split {
        left_bounds: BOUNDS3_EMPTY,
        right_bounds: BOUNDS3_EMPTY,
        axis: -1,
        index: -1,
    };

    // Compute bounds of primitive centroids and choose split axis
    let mut bounds = AABB { lower_bound: primitives[0].center, upper_bound: primitives[0].center };
    for i in 1..count {
        bounds = aabb_add_point(bounds, primitives[i as usize].center);
    }

    // Compute costs for splitting after each bucket and keep track of best split
    // This is a small O(n^2) loop. This can be further optimized, but it is already
    // very fast and is kept for simplicity right now.
    let mut best_bucket = -1;
    let mut best_cost = f32::MAX;

    for axis in 0..3 {
        let extent = aabb_extents(bounds);
        if get_by_index(extent, axis) < linear_slop() {
            continue;
        }

        // Initialize buckets
        let mut buckets = [Bucket { count: 0, bounds: BOUNDS3_EMPTY }; BIN_COUNT];

        // Fill buckets
        let factor = BIN_COUNT as f32 * (1.0 - f32::EPSILON)
            / (get_by_index(bounds.upper_bound, axis) - get_by_index(bounds.lower_bound, axis));
        for i in 0..count {
            let center = primitives[i as usize].center;
            let index = (factor * (get_by_index(center, axis) - get_by_index(bounds.lower_bound, axis))) as i32;
            b3_assert!(0 <= index && index < BIN_COUNT as i32);

            buckets[index as usize].count += 1;
            buckets[index as usize].bounds = aabb_union(buckets[index as usize].bounds, primitives[i as usize].aabb);
        }

        // Evaluate splits
        for i in 0..(BIN_COUNT - 1) {
            let mut left_count = 0;
            let mut left_bounds = BOUNDS3_EMPTY;
            for k in 0..=i {
                left_count += buckets[k].count;
                left_bounds = aabb_union(left_bounds, buckets[k].bounds);
            }

            let mut right_count = 0;
            let mut right_bounds = BOUNDS3_EMPTY;
            for k in (i + 1)..BIN_COUNT {
                right_count += buckets[k].count;
                right_bounds = aabb_union(right_bounds, buckets[k].bounds);
            }

            b3_assert!(left_count + right_count == count);
            if left_count > 0 && right_count > 0 {
                let cost = left_count as f32 * aabb_area(left_bounds) + right_count as f32 * aabb_area(right_bounds);

                if cost < best_cost {
                    best_bucket = i as i32;
                    best_cost = cost;

                    split.axis = axis;
                    split.index = left_count;
                    split.left_bounds = left_bounds;
                    split.right_bounds = right_bounds;
                }
            }
        }
    }

    // Partition
    if best_bucket >= 0 {
        let axis = split.axis;
        let factor = BIN_COUNT as f32 * (1.0 - f32::EPSILON)
            / (get_by_index(bounds.upper_bound, axis) - get_by_index(bounds.lower_bound, axis));

        let mut split_index = 0;
        for i in 0..count {
            let center = primitives[i as usize].center;
            let index = (factor * (get_by_index(center, axis) - get_by_index(bounds.lower_bound, axis))) as i32;

            if index <= best_bucket {
                primitives.swap(i as usize, split_index as usize);
                split_index += 1;
            }
        }
        b3_assert!(split_index == split.index);
    }

    split
}

fn split_half(primitives: &mut [Primitive]) -> Split {
    let count = primitives.len() as i32;

    // Split in the middle
    let split_index = count / 2;

    let mut left_bounds = BOUNDS3_EMPTY;
    for i in 0..split_index {
        left_bounds = aabb_union(left_bounds, primitives[i as usize].aabb);
    }

    let mut right_bounds = BOUNDS3_EMPTY;
    for i in split_index..count {
        right_bounds = aabb_union(right_bounds, primitives[i as usize].aabb);
    }

    let bounds = aabb_union(left_bounds, right_bounds);
    let axis = major_axis(aabb_extents(bounds));

    Split { left_bounds, right_bounds, axis, index: split_index }
}

fn split_median(primitives: &mut [Primitive]) -> Split {
    let count = primitives.len() as i32;
    b3_assert!(count > 2);

    let mut lower_bound = primitives[0].center;
    let mut upper_bound = primitives[0].center;

    for i in 1..count {
        lower_bound = min(lower_bound, primitives[i as usize].center);
        upper_bound = max(upper_bound, primitives[i as usize].center);
    }

    let d = sub(upper_bound, lower_bound);
    let c = mul_sv(0.5, add(lower_bound, upper_bound));

    let mut split = Split {
        left_bounds: BOUNDS3_EMPTY,
        right_bounds: BOUNDS3_EMPTY,
        axis: 0,
        index: -1,
    };

    // Partition longest axis using the Hoare partition scheme
    // https://en.wikipedia.org/wiki/Quicksort
    // https://nicholasvadivelu.com/2021/01/11/array-partition/
    let mut i1: i32 = 0;
    let mut i2: i32 = count;
    if d.x >= d.y && d.x >= d.z {
        split.axis = 0;

        let pivot = c.x;

        while i1 < i2 {
            while i1 < i2 && primitives[i1 as usize].center.x < pivot {
                i1 += 1;
            }

            while i1 < i2 && primitives[(i2 - 1) as usize].center.x >= pivot {
                i2 -= 1;
            }

            if i1 < i2 {
                // Swap primitives
                primitives.swap(i1 as usize, (i2 - 1) as usize);

                i1 += 1;
                i2 -= 1;
            }
        }
    } else if d.y >= d.z {
        split.axis = 1;

        let pivot = c.y;

        while i1 < i2 {
            while i1 < i2 && primitives[i1 as usize].center.y < pivot {
                i1 += 1;
            }

            while i1 < i2 && primitives[(i2 - 1) as usize].center.y >= pivot {
                i2 -= 1;
            }

            if i1 < i2 {
                // Swap primitives
                primitives.swap(i1 as usize, (i2 - 1) as usize);

                i1 += 1;
                i2 -= 1;
            }
        }
    } else {
        split.axis = 2;

        let pivot = c.z;

        while i1 < i2 {
            while i1 < i2 && primitives[i1 as usize].center.z < pivot {
                i1 += 1;
            }

            while i1 < i2 && primitives[(i2 - 1) as usize].center.z >= pivot {
                i2 -= 1;
            }

            if i1 < i2 {
                // Swap primitives
                primitives.swap(i1 as usize, (i2 - 1) as usize);

                i1 += 1;
                i2 -= 1;
            }
        }
    }
    b3_assert!(i1 == i2);
    b3_assert!(0 <= i1 && i1 < count);

    if i1 == 0 || i1 == count - 1 {
        // failed to split
        i1 = count / 2;
    }

    let mut left_bounds = BOUNDS3_EMPTY;
    for i in 0..i1 {
        left_bounds = aabb_union(left_bounds, primitives[i as usize].aabb);
    }

    let mut right_bounds = BOUNDS3_EMPTY;
    for i in i1..count {
        right_bounds = aabb_union(right_bounds, primitives[i as usize].aabb);
    }

    split.index = i1;
    split.left_bounds = left_bounds;
    split.right_bounds = right_bounds;
    split
}

fn validate_split(primitives: &[Primitive], split: &Split) -> bool {
    if split.axis < 0 {
        return false;
    }

    let count = primitives.len() as i32;

    for i in 0..split.index {
        if !aabb_contains(split.left_bounds, primitives[i as usize].aabb) {
            return false;
        }
    }

    for i in split.index..count {
        if !aabb_contains(split.right_bounds, primitives[i as usize].aabb) {
            return false;
        }
    }

    true
}

fn build_recursive(
    nodes: &mut Vec<MeshNode>,
    primitives: &mut [Primitive],
    base_offset: i32,
    use_median_split: bool,
    height: &mut i32,
) -> i32 {
    let count = primitives.len() as i32;

    if count > DESIRED_TRIANGLES_PER_LEAF {
        // Try to split the input set using the SAH
        let mut split = if use_median_split {
            split_median(primitives)
        } else {
            split_binned_sah(primitives)
        };

        if split.axis < 0 {
            if count > MAXIMUM_TRIANGLES_PER_LEAF {
                // Re-split. This is a less optimal split and can create more false positives!
                split = split_half(primitives);
            } else {
                let mut bounds = BOUNDS3_EMPTY;
                for primitive in primitives.iter() {
                    bounds = aabb_union(bounds, primitive.aabb);
                }

                // We have only a few triangles left. Create a leaf.
                let index = nodes.len() as i32;
                nodes.push(MeshNode::default());
                store_leaf(&mut nodes[index as usize], &bounds, count, base_offset);

                return index;
            }
        }
        b3_validate!(validate_split(primitives, &split));

        // Allocate node and recurse
        let index = nodes.len() as i32;
        nodes.push(MeshNode::default());

        let mut height_left = 0;
        let mut height_right = 0;
        let (left_primitives, right_primitives) = primitives.split_at_mut(split.index as usize);
        let left_index = build_recursive(nodes, left_primitives, base_offset, use_median_split, &mut height_left);
        let right_index = build_recursive(
            nodes,
            right_primitives,
            base_offset + split.index,
            use_median_split,
            &mut height_right,
        );

        *height = max_int(height_left, height_right) + 1;

        let _ = left_index;
        b3_assert!(left_index - index == 1 && right_index - index > 1);

        let aabb = aabb_union(split.left_bounds, split.right_bounds);
        let node = &mut nodes[index as usize];
        node.data = MeshNode::pack_node(split.axis as u32, (right_index - index) as u32);
        node.lower_bound = aabb.lower_bound;
        node.upper_bound = aabb.upper_bound;
        // triangleOffset is leaf-only, but lives outside the union — zero it so mesh->hash is deterministic
        node.triangle_offset = 0;

        return index;
    }

    let mut aabb = BOUNDS3_EMPTY;
    for primitive in primitives.iter() {
        aabb = aabb_union(aabb, primitive.aabb);
    }

    let index = nodes.len() as i32;
    nodes.push(MeshNode::default());
    store_leaf(&mut nodes[index as usize], &aabb, count, base_offset);

    *height = 1;

    index
}

// Sort triangles in depth-first-order.
// Casts and volume queries will return sorted arrays.
fn sort_mesh_triangles(mesh: &mut MeshData) -> bool {
    let triangle_count = mesh.triangle_count();

    let mut temp_triangles: Vec<MeshTriangle> = Vec::with_capacity(triangle_count as usize);
    let mut temp_material_indices: Vec<u8> = Vec::with_capacity(triangle_count as usize);

    let mut count = 0usize;
    let mut stack = [0i32; MESH_STACK_SIZE];
    stack[count] = 0;
    count += 1;

    let mut offset = 0;
    while count > 0 {
        count -= 1;
        let node_index = stack[count];
        let node = mesh.nodes[node_index as usize];

        if !node.is_leaf() {
            if count >= MESH_STACK_SIZE - 2 {
                return false;
            }

            stack[count] = right_child(node_index, &node);
            count += 1;
            stack[count] = left_child(node_index);
            count += 1;
        } else {
            let leaf_triangle_count = node.triangle_count() as i32;
            let triangle_offset = node.triangle_offset as i32;

            for triangle in 0..leaf_triangle_count {
                let index = (triangle_offset + triangle) as usize;
                temp_triangles.push(mesh.triangles[index]);
                temp_material_indices.push(mesh.materials[index]);
            }

            mesh.nodes[node_index as usize].triangle_offset = offset as u32;
            offset += leaf_triangle_count;
        }
    }

    b3_assert!(offset == temp_triangles.len() as i32);
    b3_assert!(temp_triangles.len() as i32 == triangle_count);
    b3_assert!(temp_material_indices.len() as i32 == triangle_count);

    // Copy sorted triangle array back to tree
    mesh.triangles.copy_from_slice(&temp_triangles);
    mesh.materials.copy_from_slice(&temp_material_indices);

    true
}

// ---------------------------------------------------------------------------
// Edge identification
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct MeshEdge {
    vertex1: i32,
    vertex2: i32,
    triangle1: i32,
    triangle2: i32,
    triangle_count: u16,

    // The index of an edge within the parent triangle: 0, 1, or 2. 0xFF is unset
    triangle_edge_index1: u8,
    triangle_edge_index2: u8,
}

fn identify_edges(mesh: &mut MeshData) {
    let triangle_count = mesh.triangle_count();
    let edge_count = 3 * triangle_count;
    let mut edges: Vec<MeshEdge> = Vec::with_capacity(edge_count as usize);
    let mut normals: Vec<Vec3> = Vec::with_capacity(triangle_count as usize);

    for i in 0..triangle_count {
        let triangle = mesh.triangles[i as usize];
        let i1 = triangle.index1;
        let i2 = triangle.index2;
        let i3 = triangle.index3;

        edges.push(MeshEdge {
            vertex1: min_int(i1, i2),
            vertex2: max_int(i1, i2),
            triangle1: i,
            triangle2: NULL_INDEX,
            triangle_count: 1,
            triangle_edge_index1: 0,
            triangle_edge_index2: 0xFF,
        });

        edges.push(MeshEdge {
            vertex1: min_int(i2, i3),
            vertex2: max_int(i2, i3),
            triangle1: i,
            triangle2: NULL_INDEX,
            triangle_count: 1,
            triangle_edge_index1: 1,
            triangle_edge_index2: 0xFF,
        });

        edges.push(MeshEdge {
            vertex1: min_int(i3, i1),
            vertex2: max_int(i3, i1),
            triangle1: i,
            triangle2: NULL_INDEX,
            triangle_count: 1,
            triangle_edge_index1: 2,
            triangle_edge_index2: 0xFF,
        });

        let v1 = mesh.vertices[i1 as usize];
        let v2 = mesh.vertices[i2 as usize];
        let v3 = mesh.vertices[i3 as usize];

        let e1 = sub(v2, v1);
        let e2 = sub(v3, v1);
        let n = cross(e1, e2);

        normals.push(normalize(n));
    }

    // Map used only for keyed lookup (never iterated) — deterministic with std HashMap.
    let mut map: HashMap<u64, i32> = HashMap::with_capacity(edge_count as usize);

    let key = ((edges[0].vertex1 as u64) << 32) | (edges[0].vertex2 as u64);
    map.insert(key, 0);

    // Find unique edges and assign adjacency
    for i in 1..edge_count {
        let edge = edges[i as usize];
        let key = ((edge.vertex1 as u64) << 32) | (edge.vertex2 as u64);

        match map.get(&key) {
            None => {
                map.insert(key, i);
            }
            Some(&other_index) => {
                b3_assert!(other_index < i);

                let base = &mut edges[other_index as usize];
                if base.triangle_count == 1 {
                    base.triangle2 = edge.triangle1;
                    base.triangle_edge_index2 = edge.triangle_edge_index1;
                }

                base.triangle_count += 1;
            }
        }
    }

    drop(map);

    for i in 0..edge_count {
        let edge = edges[i as usize];
        if edge.triangle_count != 2 {
            continue;
        }

        b3_assert!(edge.triangle_edge_index1 < 3);
        b3_assert!(edge.triangle_edge_index2 < 3);

        let triangle1 = mesh.triangles[edge.triangle1 as usize];
        let triangle2 = mesh.triangles[edge.triangle2 as usize];

        let j1 = triangle2.index1;
        let j2 = triangle2.index2;
        let j3 = triangle2.index3;

        let opposite = match edge.triangle_edge_index2 {
            0 => j3,
            1 => j1,
            2 => j2,
            _ => {
                b3_assert!(false);
                NULL_INDEX
            }
        };

        let i1 = triangle1.index1;
        let i2 = triangle1.index2;
        let i3 = triangle1.index3;

        let v1 = mesh.vertices[i1 as usize];
        let v2 = mesh.vertices[i2 as usize];
        let v3 = mesh.vertices[i3 as usize];
        let p = mesh.vertices[opposite as usize];

        let cos5_deg = 0.9962;
        let signed_vol = signed_volume(v1, v2, v3, p);
        let n1 = normals[edge.triangle1 as usize];
        let n2 = normals[edge.triangle2 as usize];
        let cos_angle = dot(n1, n2);
        if signed_vol > 0.0 || cos_angle > cos5_deg {
            let edge_flags = [CONCAVE_EDGE1, CONCAVE_EDGE2, CONCAVE_EDGE3];
            mesh.flags[edge.triangle1 as usize] |= edge_flags[edge.triangle_edge_index1 as usize] as u8;
            mesh.flags[edge.triangle2 as usize] |= edge_flags[edge.triangle_edge_index2 as usize] as u8;
        }

        if signed_vol < 0.0 || cos_angle > cos5_deg {
            let edge_flags = [INVERSE_CONCAVE_EDGE1, INVERSE_CONCAVE_EDGE2, INVERSE_CONCAVE_EDGE3];
            mesh.flags[edge.triangle1 as usize] |= edge_flags[edge.triangle_edge_index1 as usize] as u8;
            mesh.flags[edge.triangle2 as usize] |= edge_flags[edge.triangle_edge_index2 as usize] as u8;
        }
    }
}

// ---------------------------------------------------------------------------
// Mesh creation
// ---------------------------------------------------------------------------

/// The C mesh->materialCount: number of distinct materials (minimum 1).
/// The port recomputes it from the per-triangle material indices; only
/// non-degenerate triangles are stored, matching the C computation.
pub fn get_mesh_material_count(mesh: &MeshData) -> i32 {
    let mut material_count = 1;
    for &index in &mesh.materials {
        material_count = max_int(material_count, index as i32 + 1);
    }
    material_count
}

// Canonical content serialization for the mesh hash. The C code hashes the raw
// allocation; only self-consistency matters for the port.
fn compute_mesh_hash(mesh: &MeshData) -> u32 {
    let mut bytes: Vec<u8> = Vec::with_capacity(
        48 + mesh.nodes.len() * 36 + mesh.vertices.len() * 12 + mesh.triangles.len() * 12 + 2 * mesh.materials.len(),
    );

    fn push_f32(bytes: &mut Vec<u8>, v: f32) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fn push_vec3(bytes: &mut Vec<u8>, v: Vec3) {
        bytes.extend_from_slice(&v.x.to_le_bytes());
        bytes.extend_from_slice(&v.y.to_le_bytes());
        bytes.extend_from_slice(&v.z.to_le_bytes());
    }

    push_vec3(&mut bytes, mesh.bounds.lower_bound);
    push_vec3(&mut bytes, mesh.bounds.upper_bound);
    push_f32(&mut bytes, mesh.surface_area);
    bytes.extend_from_slice(&mesh.tree_height.to_le_bytes());
    bytes.extend_from_slice(&mesh.degenerate_count.to_le_bytes());

    for node in &mesh.nodes {
        push_vec3(&mut bytes, node.lower_bound);
        bytes.extend_from_slice(&node.data.to_le_bytes());
        push_vec3(&mut bytes, node.upper_bound);
        bytes.extend_from_slice(&node.triangle_offset.to_le_bytes());
    }

    for v in &mesh.vertices {
        push_vec3(&mut bytes, *v);
    }

    for t in &mesh.triangles {
        bytes.extend_from_slice(&t.index1.to_le_bytes());
        bytes.extend_from_slice(&t.index2.to_le_bytes());
        bytes.extend_from_slice(&t.index3.to_le_bytes());
    }

    bytes.extend_from_slice(&mesh.materials);
    bytes.extend_from_slice(&mesh.flags);

    non_zero_hash(hash(HASH_INIT, &bytes))
}

/// Create a generic mesh.
/// Returns None on invalid input, insane bounds, or BVH height overflow.
/// Geometric degenerate triangle indices are pushed (unbounded) into the
/// optional output Vec; the count is recorded in MeshData::degenerate_count.
// todo this should fail if the mesh has a height greater than B3_MESH_STACK_SIZE
pub fn create_mesh(def: &MeshDef, mut degenerate_triangle_indices: Option<&mut Vec<i32>>) -> Option<Arc<MeshData>> {
    let def_vertex_count = def.vertices.len() as i32;
    let def_triangle_count = (def.indices.len() / 3) as i32;

    if def_vertex_count < 3 || def_triangle_count <= 0 {
        return None;
    }

    let mut triangle_count = def_triangle_count;
    let mut vertex_count = def_vertex_count;

    let mut mesh_bounds = BOUNDS3_EMPTY;

    // Clone indices and vertices to support welding
    let vertices: Vec<Vec3>;
    let indices: Vec<i32>;

    if def.weld_vertices && def.weld_tolerance > 0.0 {
        let mut dst_vertices = vec![Vec3::ZERO; vertex_count as usize];
        let mut dst_indices = vec![0i32; (3 * triangle_count) as usize];
        let unique_count =
            weld_vertices(def.vertices, def.indices, &mut dst_vertices, &mut dst_indices, def.weld_tolerance);
        dst_vertices.truncate(unique_count as usize);
        vertices = dst_vertices;
        indices = dst_indices;
        vertex_count = unique_count;
        b3_assert!(vertex_count <= def_vertex_count);
    } else {
        vertices = def.vertices.to_vec();
        indices = def.indices.to_vec();
    }

    let _ = vertex_count;

    let mut primitives: Vec<Primitive> = Vec::with_capacity(triangle_count as usize);
    let mut degenerate_count = 0;
    let min_area = 0.01 * linear_slop() * linear_slop();
    let mut surface_area = 0.0;

    for index in 0..triangle_count {
        let index1 = indices[(3 * index) as usize];
        let index2 = indices[(3 * index + 1) as usize];
        let index3 = indices[(3 * index + 2) as usize];

        let vertex1 = vertices[index1 as usize];
        let vertex2 = vertices[index2 as usize];
        let vertex3 = vertices[index3 as usize];

        let normal = cross(sub(vertex2, vertex1), sub(vertex3, vertex1));
        let area = 0.5 * length(normal);

        if area < min_area {
            if index1 != index2 && index1 != index3 && index2 != index3 {
                degenerate_count += 1;
                if let Some(out) = degenerate_triangle_indices.as_deref_mut() {
                    out.push(index);
                }
            }

            continue;
        }

        surface_area += area;

        let bounds = AABB {
            lower_bound: min(vertex1, min(vertex2, vertex3)),
            upper_bound: max(vertex1, max(vertex2, vertex3)),
        };

        let center = aabb_center(bounds);

        primitives.push(Primitive { aabb: bounds, center, triangle_index: index });

        mesh_bounds = aabb_union(mesh_bounds, bounds);
    }

    // Update triangle count due to degenerates being skipped
    triangle_count = primitives.len() as i32;

    if !is_sane_aabb(mesh_bounds) {
        return None;
    }

    // Build the tree (this reorders the builder triangles)
    let mut temp_nodes: Vec<MeshNode> = Vec::with_capacity((2 * triangle_count - 1) as usize);

    let mut tree_height = 0;
    build_recursive(&mut temp_nodes, &mut primitives, 0, def.use_median_split, &mut tree_height);

    let mut mesh = MeshData {
        hash: 0,
        bounds: mesh_bounds,
        surface_area,
        tree_height,
        degenerate_count,
        nodes: temp_nodes,
        vertices,
        triangles: Vec::with_capacity(triangle_count as usize),
        materials: vec![0u8; triangle_count as usize],
        flags: vec![0u8; triangle_count as usize],
    };

    for index in 0..triangle_count {
        let primitive = primitives[index as usize];
        mesh.triangles.push(MeshTriangle {
            index1: indices[(3 * primitive.triangle_index) as usize],
            index2: indices[(3 * primitive.triangle_index + 1) as usize],
            index3: indices[(3 * primitive.triangle_index + 2) as usize],
        });

        // Copy material indices if they exist. Otherwise the material indices are all zeroes.
        if !def.material_indices.is_empty() {
            mesh.materials[index as usize] = def.material_indices[primitive.triangle_index as usize];
        }
    }

    // Sort triangles in DFS order. Casts and volume queries will return sorted arrays.
    // This also sorts material indices, but not the materials.
    // This can fail if the BVH height is too large.
    let success = sort_mesh_triangles(&mut mesh);
    if !success {
        return None;
    }

    if def.identify_edges {
        identify_edges(&mut mesh);
    }

    b3_validate!(is_non_degenerate(&mesh, min_area));
    b3_validate!(is_consistent(&mesh));

    mesh.hash = compute_mesh_hash(&mesh);

    Some(Arc::new(mesh))
}

/// Create a grid mesh along the x and z axes.
pub fn create_grid_mesh(
    x_count: i32,
    z_count: i32,
    cell_width: f32,
    material_count: i32,
    identify_edges: bool,
) -> Arc<MeshData> {
    b3_assert!(0 <= material_count && material_count <= u8::MAX as i32);

    // Create vertices
    let vertex_count = (x_count + 1) * (z_count + 1);

    let mut vertices = vec![Vec3::ZERO; vertex_count as usize];
    let mut index = 0;

    let x_width = cell_width * x_count as f32;
    let z_width = cell_width * z_count as f32;

    let mut x = -0.5 * x_width;
    for _ix in 0..=x_count {
        let mut z = -0.5 * z_width;
        for _iz in 0..=z_count {
            vertices[index as usize] = vec3(x, 0.0, z);
            z += cell_width;
            index += 1;
        }
        x += cell_width;
    }
    b3_assert!(index == vertex_count);

    // Triangles
    let triangle_count = 2 * x_count * z_count;

    let mut indices = vec![0i32; (3 * triangle_count) as usize];
    let mut material_indices = vec![0u8; triangle_count as usize];

    let mut material_index = 0;
    index = 0;
    for ix in 0..x_count {
        for iz in 0..z_count {
            let index1 = iz + (z_count + 1) * ix;
            let index2 = index1 + 1;
            let index3 = index2 + (z_count + 1);
            let index4 = index3 - 1;

            b3_assert!(index1 < vertex_count);
            b3_assert!(index2 < vertex_count);
            b3_assert!(index3 < vertex_count);
            b3_assert!(index4 < vertex_count);

            indices[index as usize] = index1;
            indices[(index + 1) as usize] = index2;
            indices[(index + 2) as usize] = index3;

            indices[(index + 3) as usize] = index3;
            indices[(index + 4) as usize] = index4;
            indices[(index + 5) as usize] = index1;

            if material_count > 0 {
                material_indices[(2 * material_index) as usize] = (material_index % material_count) as u8;
                material_indices[(2 * material_index + 1) as usize] = (material_index % material_count) as u8;
            }

            material_index += 1;
            index += 6;
        }
    }
    b3_assert!(index == 3 * triangle_count);

    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        material_indices: if material_count > 0 { &material_indices } else { &[] },
        use_median_split: true,
        identify_edges,
        ..Default::default()
    };

    create_mesh(&def, None).expect("create_grid_mesh failed")
}

/// Create a wave mesh along the x and z axes.
pub fn create_wave_mesh(
    x_count: i32,
    z_count: i32,
    cell_width: f32,
    amplitude: f32,
    row_frequency: f32,
    column_frequency: f32,
) -> Arc<MeshData> {
    // Create vertices
    let vertex_count = (x_count + 1) * (z_count + 1);

    let mut vertices = vec![Vec3::ZERO; vertex_count as usize];
    let mut index = 0;

    let x_width = cell_width * x_count as f32;
    let z_width = cell_width * z_count as f32;

    let omega_z = TWO_PI * row_frequency * cell_width;
    let omega_x = TWO_PI * column_frequency * cell_width;

    let mut x = -0.5 * x_width;
    for ix in 0..=x_count {
        // The C code uses libm sinf here
        let row_height = (omega_x * ix as f32).sin();

        let mut z = -0.5 * z_width;
        for iz in 0..=z_count {
            let column_height = (omega_z * iz as f32).sin();

            let y = amplitude * row_height * column_height;
            vertices[index as usize] = vec3(x, y, z);
            z += cell_width;
            index += 1;
        }
        x += cell_width;
    }
    b3_assert!(index == vertex_count);

    // Triangles
    let triangle_count = 2 * x_count * z_count;

    let mut indices = vec![0i32; (3 * triangle_count) as usize];

    index = 0;
    for ix in 0..x_count {
        for iz in 0..z_count {
            let index1 = iz + (z_count + 1) * ix;
            let index2 = index1 + 1;
            let index3 = index2 + (z_count + 1);
            let index4 = index3 - 1;

            b3_assert!(index1 < vertex_count);
            b3_assert!(index2 < vertex_count);
            b3_assert!(index3 < vertex_count);
            b3_assert!(index4 < vertex_count);

            indices[index as usize] = index1;
            indices[(index + 1) as usize] = index2;
            indices[(index + 2) as usize] = index3;

            indices[(index + 3) as usize] = index3;
            indices[(index + 4) as usize] = index4;
            indices[(index + 5) as usize] = index1;

            index += 6;
        }
    }
    b3_assert!(index == 3 * triangle_count);

    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        use_median_split: true,
        identify_edges: true,
        ..Default::default()
    };

    create_mesh(&def, None).expect("create_wave_mesh failed")
}

/// Create a torus mesh.
pub fn create_torus_mesh(radial_resolution: i32, tubular_resolution: i32, radius: f32, thickness: f32) -> Arc<MeshData> {
    // Create vertices
    let mut vertices: Vec<Vec3> = Vec::new();

    for radial_index in 0..radial_resolution {
        for tubular_index in 0..tubular_resolution {
            let u = tubular_index as f32 / tubular_resolution as f32 * TWO_PI;
            let v = radial_index as f32 / radial_resolution as f32 * TWO_PI;

            let x = (radius + thickness * cos(v)) * cos(u);
            let y = (radius + thickness * cos(v)) * sin(u);
            let z = thickness * sin(v);

            vertices.push(vec3(x, y, z));
        }
    }

    // Triangles
    let mut indices: Vec<i32> = Vec::new();
    for radial_index1 in 0..radial_resolution {
        let radial_index2 = (radial_index1 + 1) % radial_resolution;
        for tubular_index1 in 0..tubular_resolution {
            let tubular_index2 = (tubular_index1 + 1) % tubular_resolution;
            let index1 = radial_index1 * tubular_resolution + tubular_index1;
            let index2 = radial_index1 * tubular_resolution + tubular_index2;
            let index3 = radial_index2 * tubular_resolution + tubular_index2;
            let index4 = radial_index2 * tubular_resolution + tubular_index1;

            indices.push(index1);
            indices.push(index2);
            indices.push(index3);

            indices.push(index3);
            indices.push(index4);
            indices.push(index1);
        }
    }

    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        use_median_split: false,
        identify_edges: true,
        ..Default::default()
    };

    create_mesh(&def, None).expect("create_torus_mesh failed")
}

/// Create a box mesh.
pub fn create_box_mesh(center: Vec3, extent: Vec3, identify_edges: bool) -> Arc<MeshData> {
    let x = extent.x;
    let y = extent.y;
    let z = extent.z;
    let mut vertices = [
        vec3(x, y, z),
        vec3(-x, y, z),
        vec3(-x, -y, z),
        vec3(x, -y, z),
        vec3(x, y, -z),
        vec3(-x, y, -z),
        vec3(-x, -y, -z),
        vec3(x, -y, -z),
    ];

    for v in vertices.iter_mut() {
        *v = add(*v, center);
    }

    let indices: [i32; 36] = [
        0, 1, 3, 1, 2, 3, // front
        0, 4, 1, 1, 4, 5, // top
        0, 3, 7, 4, 0, 7, // right
        4, 7, 5, 6, 5, 7, // back
        1, 5, 2, 6, 2, 5, // left
        3, 2, 7, 6, 7, 2, // bottom
    ];

    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        use_median_split: false,
        identify_edges,
        ..Default::default()
    };

    create_mesh(&def, None).expect("create_box_mesh failed")
}

/// Create a hollow box mesh (triangles wound inward).
pub fn create_hollow_box_mesh(center: Vec3, extent: Vec3) -> Arc<MeshData> {
    let x = extent.x;
    let y = extent.y;
    let z = extent.z;
    let mut vertices = [
        vec3(x, y, z),
        vec3(-x, y, z),
        vec3(-x, -y, z),
        vec3(x, -y, z),
        vec3(x, y, -z),
        vec3(-x, y, -z),
        vec3(-x, -y, -z),
        vec3(x, -y, -z),
    ];

    for v in vertices.iter_mut() {
        *v = add(*v, center);
    }

    let indices: [i32; 36] = [
        3, 1, 0, 3, 2, 1, // front
        1, 4, 0, 5, 4, 1, // top
        7, 3, 0, 7, 0, 4, // right
        5, 7, 4, 7, 5, 6, // back
        2, 5, 1, 5, 2, 6, // left
        7, 2, 3, 2, 7, 6, // bottom
    ];

    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        use_median_split: false,
        identify_edges: true,
        ..Default::default()
    };

    create_mesh(&def, None).expect("create_hollow_box_mesh failed")
}

/// Create a platform mesh. A truncated pyramid.
pub fn create_platform_mesh(center: Vec3, height: f32, top_width: f32, bottom_width: f32) -> Arc<MeshData> {
    let hb = 0.5 * bottom_width;
    let ht = 0.5 * top_width;
    let hy = 0.5 * height;
    let mut vertices = [
        vec3(ht, hy, ht),
        vec3(-ht, hy, ht),
        vec3(-hb, -hy, hb),
        vec3(hb, -hy, hb),
        vec3(ht, hy, -ht),
        vec3(-ht, hy, -ht),
        vec3(-hb, -hy, -hb),
        vec3(hb, -hy, -hb),
    ];

    for v in vertices.iter_mut() {
        *v = add(*v, center);
    }

    let indices: [i32; 36] = [
        0, 1, 3, 1, 2, 3, // front
        0, 4, 1, 1, 4, 5, // top
        0, 3, 7, 4, 0, 7, // right
        4, 7, 5, 6, 5, 7, // back
        1, 5, 2, 6, 2, 5, // left
        3, 2, 7, 6, 7, 2, // bottom
    ];

    let def = MeshDef {
        vertices: &vertices,
        indices: &indices,
        use_median_split: true,
        identify_edges: true,
        ..Default::default()
    };

    create_mesh(&def, None).expect("create_platform_mesh failed")
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Overlap shape versus mesh.
pub fn overlap_mesh(shape: &Mesh, shape_transform: Transform, proxy: &ShapeProxy) -> bool {
    b3_assert!(proxy.count() > 0);
    let mut cache = SimplexCache::default();

    let mut buffer = [Vec3::ZERO; MAX_SHAPE_CAST_POINTS];
    let local_proxy = crate::shape::make_local_proxy(proxy, shape_transform, &mut buffer);
    let aabb = crate::shape::compute_proxy_aabb(&local_proxy);

    let mesh_scale = shape.scale;

    // Scale may have reflection so min/max may become invalid when unscaled
    let inv_scale = div_v(Vec3::ONE, mesh_scale);
    let temp1 = mul(inv_scale, aabb.lower_bound);
    let temp2 = mul(inv_scale, aabb.upper_bound);
    let inv_scaled_bounds_min = min(temp1, temp2);
    let inv_scaled_bounds_max = max(temp1, temp2);
    let inv_scaled_bounds_center = mul_sv(0.5, add(inv_scaled_bounds_min, inv_scaled_bounds_max));
    let inv_scaled_bounds_extent = sub(inv_scaled_bounds_max, inv_scaled_bounds_center);

    let data = &*shape.data;
    if data.nodes.is_empty() {
        return false;
    }

    let mut count = 0usize;
    let mut stack = [0i32; MESH_STACK_SIZE];
    let mut node_index = 0i32;

    loop {
        let node = &data.nodes[node_index as usize];

        // Test node overlap in unscaled space
        if test_bounds_overlap(node.lower_bound, node.upper_bound, inv_scaled_bounds_min, inv_scaled_bounds_max)
        {
            if node.is_leaf() {
                let triangle_count = node.triangle_count() as i32;
                let triangle_offset = node.triangle_offset as i32;

                for index in 0..triangle_count {
                    let triangle_index = triangle_offset + index;
                    let triangle = data.triangles[triangle_index as usize];

                    let vertex1 = data.vertices[triangle.index1 as usize];
                    let vertex2 = data.vertices[triangle.index2 as usize];
                    let vertex3 = data.vertices[triangle.index3 as usize];

                    // Bounding box overlap test in unscaled space
                    if test_bounds_triangle_overlap(
                        inv_scaled_bounds_center,
                        inv_scaled_bounds_extent,
                        vertex1,
                        vertex2,
                        vertex3,
                    ) {
                        // Shape-triangle overlap test in scaled space. Winding order doesn't matter.
                        let triangle_vertices =
                            [mul(mesh_scale, vertex1), mul(mesh_scale, vertex2), mul(mesh_scale, vertex3)];

                        let input = DistanceInput {
                            proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                            proxy_b: local_proxy,
                            transform: Transform::IDENTITY,
                            use_radii: true,
                        };

                        // reset the cache
                        cache.count = 0;

                        // get distance between triangle and query shape
                        let output = crate::distance::shape_distance(&input, &mut cache, None);

                        let tolerance = 0.1 * linear_slop();
                        if output.distance < tolerance {
                            // overlap detected
                            return true;
                        }
                    }
                }
            } else {
                // Recurse
                b3_assert!(count <= MESH_STACK_SIZE - 1);
                stack[count] = right_child(node_index, node);
                count += 1;
                node_index = left_child(node_index);

                continue;
            }
        }

        if count == 0 {
            break;
        }
        count -= 1;
        node_index = stack[count];
    }

    false
}

/// Compute the bounding box of a transformed mesh. Scale may be non-uniform and
/// have negative components.
pub fn compute_mesh_aabb(shape: &MeshData, transform: Transform, scale: Vec3) -> AABB {
    let scaled_lower = mul(scale, shape.bounds.lower_bound);
    let scaled_upper = mul(scale, shape.bounds.upper_bound);
    let bounds = AABB {
        lower_bound: min(scaled_lower, scaled_upper),
        upper_bound: max(scaled_lower, scaled_upper),
    };
    aabb_transform(transform, bounds)
}

/// Ray cast versus mesh in local space. A thin surface with no interior, so
/// there is no overlap case.
pub fn ray_cast_mesh(mesh: &Mesh, input: &RayCastInput) -> CastOutput {
    let data = &*mesh.data;
    let mesh_scale = mesh.scale;

    let mut best_output = CastOutput::default();
    best_output.fraction = input.max_fraction;
    best_output.triangle_index = NULL_INDEX;

    if data.nodes.is_empty() {
        return best_output;
    }

    let mut lambda = input.max_fraction;

    let ray_start = input.origin;
    let ray_delta = input.translation;

    let inv_scale = div_v(Vec3::ONE, mesh_scale);
    let clockwise = mesh_scale.x * mesh_scale.y * mesh_scale.z < 0.0;

    // Use the inverse scaled ray for traversal of the BVH
    let inv_scaled_ray_start = mul(inv_scale, ray_start);
    let inv_scaled_ray_delta = mul(inv_scale, ray_delta);
    let mut inv_scaled_ray_end = add(inv_scaled_ray_start, mul_sv(lambda, inv_scaled_ray_delta));
    let mut inv_scaled_ray_min = min(inv_scaled_ray_start, inv_scaled_ray_end);
    let mut inv_scaled_ray_max = max(inv_scaled_ray_start, inv_scaled_ray_end);

    let mut count = 0usize;
    let mut stack = [0i32; MESH_STACK_SIZE];
    let mut node_index = 0i32;

    loop {
        let node = &data.nodes[node_index as usize];

        // Test node/ray overlap using SAT
        if test_bounds_overlap(node.lower_bound, node.upper_bound, inv_scaled_ray_min, inv_scaled_ray_max)
            && test_bounds_ray_overlap(
                node.lower_bound,
                node.upper_bound,
                inv_scaled_ray_start,
                inv_scaled_ray_delta,
            )
        {
            // SAT: The node and ray overlap - process leaf node or recurse
            if node.is_leaf() {
                let triangle_count = node.triangle_count() as i32;
                let triangle_offset = node.triangle_offset as i32;

                for index in 0..triangle_count {
                    let triangle_index = triangle_offset + index;
                    let triangle = data.triangles[triangle_index as usize];

                    // Collide ray with triangle in scaled space
                    let vertex1 = mul(mesh_scale, data.vertices[triangle.index1 as usize]);
                    let vertex2;
                    let vertex3;

                    // The CPU should predict this branch
                    if clockwise {
                        vertex2 = mul(mesh_scale, data.vertices[triangle.index3 as usize]);
                        vertex3 = mul(mesh_scale, data.vertices[triangle.index2 as usize]);
                    } else {
                        vertex2 = mul(mesh_scale, data.vertices[triangle.index2 as usize]);
                        vertex3 = mul(mesh_scale, data.vertices[triangle.index3 as usize]);
                    }

                    let alpha = intersect_ray_triangle(ray_start, ray_delta, vertex1, vertex2, vertex3);
                    b3_assert!(0.0 <= alpha && alpha <= 1.0);

                    if alpha < best_output.fraction {
                        let edge1 = sub(vertex2, vertex1);
                        let edge2 = sub(vertex3, vertex1);
                        best_output.normal = normalize(cross(edge1, edge2));
                        best_output.point = add(input.origin, mul_sv(alpha, input.translation));
                        best_output.fraction = alpha;
                        best_output.triangle_index = triangle_index;
                        best_output.material_index = data.materials[triangle_index as usize] as i32;
                        best_output.hit = true;

                        // Update ray bounds in unscaled space
                        lambda = alpha;
                        inv_scaled_ray_end = add(inv_scaled_ray_start, mul_sv(lambda, inv_scaled_ray_delta));
                        inv_scaled_ray_min = min(inv_scaled_ray_start, inv_scaled_ray_end);
                        inv_scaled_ray_max = max(inv_scaled_ray_start, inv_scaled_ray_end);
                    }
                }
            } else {
                // Determine traversal order (front -> back) and recurse
                let axis = node.axis() as i32;
                if get_by_index(inv_scaled_ray_delta, axis) > 0.0 {
                    b3_assert!(count <= MESH_STACK_SIZE - 1);
                    stack[count] = right_child(node_index, node);
                    count += 1;
                    node_index = left_child(node_index);
                } else {
                    b3_assert!(count <= MESH_STACK_SIZE - 1);
                    stack[count] = left_child(node_index);
                    count += 1;
                    node_index = right_child(node_index, node);
                }

                continue;
            }
        }

        if count == 0 {
            break;
        }
        count -= 1;
        node_index = stack[count];
    }

    best_output
}

/// Shape cast versus a mesh. Initial overlap is treated as a miss.
pub fn shape_cast_mesh(mesh: &Mesh, input: &ShapeCastInput) -> CastOutput {
    let data = &*mesh.data;
    let mesh_scale = mesh.scale;

    let mut best_output = CastOutput::default();
    best_output.fraction = input.max_fraction;
    best_output.triangle_index = NULL_INDEX;

    if data.nodes.is_empty() {
        return best_output;
    }

    let mut lambda = input.max_fraction;

    let shape_bounds = make_aabb(input.proxy.points, input.proxy.radius);
    let center = aabb_center(shape_bounds);
    let extents = aabb_extents(shape_bounds);
    let shape_extent = extents;

    let ray_start = center;
    let ray_delta = input.translation;
    let mut ray_end = add(ray_start, mul_sv(lambda, ray_delta));
    let mut ray_min = min(ray_start, ray_end);
    let mut ray_max = max(ray_start, ray_end);

    let inv_scale = div_v(Vec3::ONE, mesh_scale);
    let abs_inv_scale = crate::math_functions::abs(inv_scale);
    let clockwise = mesh_scale.x * mesh_scale.y * mesh_scale.z < 0.0;

    // Use the inverse scaled shape cast for traversal of the BVH
    let inv_scaled_ray_start = mul(inv_scale, ray_start);
    let inv_scaled_ray_delta = mul(inv_scale, ray_delta);
    let mut inv_scaled_ray_end = add(inv_scaled_ray_start, mul_sv(lambda, inv_scaled_ray_delta));
    let mut inv_scaled_ray_min = min(inv_scaled_ray_start, inv_scaled_ray_end);
    let mut inv_scaled_ray_max = max(inv_scaled_ray_start, inv_scaled_ray_end);
    let inv_scaled_shape_extent = mul(abs_inv_scale, shape_extent);

    let mut count = 0usize;
    let mut stack = [0i32; MESH_STACK_SIZE];
    let mut node_index = 0i32;

    loop {
        let node = &data.nodes[node_index as usize];

        // Test node/ray overlap using SAT in unscaled space
        let node_min = sub(node.lower_bound, inv_scaled_shape_extent);
        let node_max = add(node.upper_bound, inv_scaled_shape_extent);

        if test_bounds_overlap(node_min, node_max, inv_scaled_ray_min, inv_scaled_ray_max)
            && test_bounds_ray_overlap(node_min, node_max, inv_scaled_ray_start, inv_scaled_ray_delta)
        {
            // SAT: The node and ray overlap - process leaf node or recurse
            if node.is_leaf() {
                let triangle_count = node.triangle_count() as i32;
                let triangle_offset = node.triangle_offset as i32;

                for index in 0..triangle_count {
                    let triangle_index = triangle_offset + index;
                    let triangle = data.triangles[triangle_index as usize];

                    // Collide ray with triangle in scaled space
                    let vertex1 = mul(mesh_scale, data.vertices[triangle.index1 as usize]);
                    let vertex2;
                    let vertex3;

                    // The CPU should predict this branch
                    if clockwise {
                        vertex2 = mul(mesh_scale, data.vertices[triangle.index3 as usize]);
                        vertex3 = mul(mesh_scale, data.vertices[triangle.index2 as usize]);
                    } else {
                        vertex2 = mul(mesh_scale, data.vertices[triangle.index2 as usize]);
                        vertex3 = mul(mesh_scale, data.vertices[triangle.index3 as usize]);
                    }

                    let triangle_min = sub(min(vertex1, min(vertex2, vertex3)), shape_extent);
                    let triangle_max = add(max(vertex1, max(vertex2, vertex3)), shape_extent);

                    // Test triangle-ray overlap in scaled space
                    if test_bounds_overlap(triangle_min, triangle_max, ray_min, ray_max) {
                        // Collide shape with triangle in scaled space
                        let origin = vertex1;
                        let triangle_vertices = [Vec3::ZERO, sub(vertex2, origin), sub(vertex3, origin)];
                        let shifted_origin = Transform { p: neg(origin), q: Quat::IDENTITY };

                        let pair_input = ShapeCastPairInput {
                            proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                            proxy_b: input.proxy,
                            transform: shifted_origin,
                            translation_b: input.translation,
                            max_fraction: best_output.fraction,
                            can_encroach: input.can_encroach,
                        };

                        let mut pair_output = crate::distance::shape_cast(&pair_input);

                        if pair_output.hit {
                            pair_output.point = add(pair_output.point, origin);

                            best_output = pair_output;
                            best_output.triangle_index = triangle_index;
                            best_output.material_index = data.materials[triangle_index as usize] as i32;

                            // Update ray bounds in scaled space
                            lambda = pair_output.fraction;
                            ray_end = add(ray_start, mul_sv(lambda, ray_delta));
                            ray_min = min(ray_start, ray_end);
                            ray_max = max(ray_start, ray_end);

                            // Ray bounds in unscaled space
                            inv_scaled_ray_end = add(inv_scaled_ray_start, mul_sv(lambda, inv_scaled_ray_delta));
                            inv_scaled_ray_min = min(inv_scaled_ray_start, inv_scaled_ray_end);
                            inv_scaled_ray_max = max(inv_scaled_ray_start, inv_scaled_ray_end);
                        }
                    }
                }
            } else {
                // Determine traversal order (front -> back) and recurse
                let axis = node.axis() as i32;
                if get_by_index(inv_scaled_ray_delta, axis) > 0.0 {
                    b3_assert!(count <= MESH_STACK_SIZE - 1);
                    stack[count] = right_child(node_index, node);
                    count += 1;
                    node_index = left_child(node_index);
                } else {
                    b3_assert!(count <= MESH_STACK_SIZE - 1);
                    stack[count] = left_child(node_index);
                    count += 1;
                    node_index = right_child(node_index, node);
                }

                continue;
            }
        }

        if count == 0 {
            break;
        }
        count -= 1;
        node_index = stack[count];
    }

    best_output
}

/// Get a scaled triangle from a mesh, correcting winding and edge flags for
/// mirroring scale.
pub fn get_mesh_triangle(mesh: &Mesh, triangle_index: i32) -> Triangle {
    b3_assert!(0 <= triangle_index && triangle_index < mesh.data.triangle_count());

    let data = &*mesh.data;
    let triangle = data.triangles[triangle_index as usize];
    let triangle_flags = data.flags[triangle_index as usize] as i32;

    let scale = mesh.scale;

    let mut result = Triangle {
        vertices: [Vec3::ZERO; 3],
        i1: triangle.index1,
        i2: 0,
        i3: 0,
        flags: 0,
    };

    result.vertices[0] = mul(scale, data.vertices[triangle.index1 as usize]);

    if scale.x * scale.y * scale.z < 0.0 {
        result.vertices[1] = mul(scale, data.vertices[triangle.index3 as usize]);
        result.vertices[2] = mul(scale, data.vertices[triangle.index2 as usize]);

        result.i2 = triangle.index3;
        result.i3 = triangle.index2;

        // mesh is inverted, so concave edges are now convex
        result.flags = 0;
        result.flags |= if (triangle_flags & INVERSE_CONCAVE_EDGE1) != 0 { CONCAVE_EDGE1 } else { 0 };
        result.flags |= if (triangle_flags & INVERSE_CONCAVE_EDGE2) != 0 { CONCAVE_EDGE2 } else { 0 };
        result.flags |= if (triangle_flags & INVERSE_CONCAVE_EDGE3) != 0 { CONCAVE_EDGE3 } else { 0 };
    } else {
        result.vertices[1] = mul(scale, data.vertices[triangle.index2 as usize]);
        result.vertices[2] = mul(scale, data.vertices[triangle.index3 as usize]);

        result.i2 = triangle.index2;
        result.i3 = triangle.index3;
        result.flags = triangle_flags;
    }

    result
}

/// Collide a character mover capsule with a mesh. Returns the number of planes
/// found. The plane capacity is planes.len().
pub fn collide_mover_and_mesh(planes: &mut [PlaneResult], shape: &Mesh, mover: &Capsule) -> i32 {
    let capacity = planes.len() as i32;
    if capacity == 0 {
        return 0;
    }

    let mover_points = [mover.center1, mover.center2];

    let mut cache = SimplexCache::default();
    let radius = mover.radius;

    let r = vec3(radius, radius, radius);
    let bounds_min = sub(min(mover.center1, mover.center2), r);
    let bounds_max = add(max(mover.center1, mover.center2), r);

    // Scale may have reflection so min/max may become invalid when unscaled
    let mesh_scale = shape.scale;
    let inv_scale = div_v(Vec3::ONE, mesh_scale);
    let temp1 = mul(inv_scale, bounds_min);
    let temp2 = mul(inv_scale, bounds_max);
    let inv_scaled_bounds_min = min(temp1, temp2);
    let inv_scaled_bounds_max = max(temp1, temp2);
    let inv_scaled_bounds_center = mul_sv(0.5, add(inv_scaled_bounds_min, inv_scaled_bounds_max));
    let inv_scaled_bounds_extent = sub(inv_scaled_bounds_max, inv_scaled_bounds_center);

    let data = &*shape.data;
    if data.nodes.is_empty() {
        return 0;
    }

    let mut count = 0usize;
    let mut stack = [0i32; MESH_STACK_SIZE];
    let mut node_index = 0i32;

    let mut plane_count = 0;
    while plane_count < capacity {
        let node = &data.nodes[node_index as usize];

        // Test node overlap in unscaled space
        if test_bounds_overlap(node.lower_bound, node.upper_bound, inv_scaled_bounds_min, inv_scaled_bounds_max)
        {
            if node.is_leaf() {
                let triangle_count = node.triangle_count() as i32;
                let triangle_offset = node.triangle_offset as i32;

                for index in 0..triangle_count {
                    let triangle_index = triangle_offset + index;
                    let triangle = data.triangles[triangle_index as usize];

                    let vertex1 = data.vertices[triangle.index1 as usize];
                    let vertex2 = data.vertices[triangle.index2 as usize];
                    let vertex3 = data.vertices[triangle.index3 as usize];

                    // Test triangle bounds overlap in unscaled space
                    if test_bounds_triangle_overlap(
                        inv_scaled_bounds_center,
                        inv_scaled_bounds_extent,
                        vertex1,
                        vertex2,
                        vertex3,
                    ) {
                        // Compute shape distance in scaled space. Winding order doesn't matter.
                        // todo implement one-sided collision?
                        let triangle_vertices =
                            [mul(mesh_scale, vertex1), mul(mesh_scale, vertex2), mul(mesh_scale, vertex3)];

                        let distance_input = DistanceInput {
                            proxy_a: ShapeProxy { points: &triangle_vertices, radius: 0.0 },
                            proxy_b: ShapeProxy { points: &mover_points, radius: 0.0 },
                            transform: Transform::IDENTITY,
                            use_radii: false,
                        };

                        // reset the cache
                        cache.count = 0;

                        // get distance between triangle and query shape
                        let distance_output = crate::distance::shape_distance(&distance_input, &mut cache, None);

                        if distance_output.distance == 0.0 {
                            // todo SAT
                        } else if distance_output.distance <= mover.radius {
                            let plane = Plane {
                                normal: distance_output.normal,
                                offset: mover.radius - distance_output.distance,
                            };
                            planes[plane_count as usize] = PlaneResult { plane, point: distance_output.point_a };
                            plane_count += 1;

                            if plane_count == capacity {
                                return plane_count;
                            }
                        }
                    }
                }
            } else {
                // Recurse
                b3_assert!(count <= MESH_STACK_SIZE - 1);
                stack[count] = right_child(node_index, node);
                count += 1;
                node_index = left_child(node_index);

                continue;
            }
        }

        if count == 0 {
            break;
        }
        count -= 1;
        node_index = stack[count];
    }

    plane_count
}

/// Query a mesh for triangles overlapping a bounding box in local space.
/// May have false positives. Useful for debug draw.
/// Return false from the callback to terminate the query.
pub fn query_mesh(mesh: &Mesh, bounds: AABB, fcn: &mut dyn FnMut(Vec3, Vec3, Vec3, i32) -> bool) {
    let mesh_scale = mesh.scale;
    let clockwise = mesh_scale.x * mesh_scale.y * mesh_scale.z > 0.0;

    // Scale may have reflection so min/max may become invalid when unscaled
    let inv_scale = div_v(Vec3::ONE, mesh_scale);
    let temp1 = mul(inv_scale, bounds.lower_bound);
    let temp2 = mul(inv_scale, bounds.upper_bound);
    let inv_scaled_bounds_min = min(temp1, temp2);
    let inv_scaled_bounds_max = max(temp1, temp2);
    let inv_scaled_bounds_center = mul_sv(0.5, add(inv_scaled_bounds_min, inv_scaled_bounds_max));
    let inv_scaled_bounds_extent = sub(inv_scaled_bounds_max, inv_scaled_bounds_center);

    let data = &*mesh.data;
    if data.nodes.is_empty() {
        return;
    }

    let mut count = 0usize;
    let mut stack = [0i32; MESH_STACK_SIZE];
    let mut node_index = 0i32;

    loop {
        let node = &data.nodes[node_index as usize];

        // Test node overlap in unscaled space
        if test_bounds_overlap(node.lower_bound, node.upper_bound, inv_scaled_bounds_min, inv_scaled_bounds_max)
        {
            if node.is_leaf() {
                let triangle_count = node.triangle_count() as i32;
                let triangle_offset = node.triangle_offset as i32;

                for index in 0..triangle_count {
                    let triangle_index = triangle_offset + index;
                    let triangle = data.triangles[triangle_index as usize];

                    let vertex1 = data.vertices[triangle.index1 as usize];
                    let vertex2 = data.vertices[triangle.index2 as usize];
                    let vertex3 = data.vertices[triangle.index3 as usize];

                    // Perform triangle overlap test in unscaled space. Winding order doesn't matter.
                    // todo it is possible that some margins are getting scaled
                    if test_bounds_triangle_overlap(
                        inv_scaled_bounds_center,
                        inv_scaled_bounds_extent,
                        vertex1,
                        vertex2,
                        vertex3,
                    ) {
                        let a = mul(mesh_scale, vertex1);
                        let b;
                        let c;
                        if clockwise {
                            b = mul(mesh_scale, vertex2);
                            c = mul(mesh_scale, vertex3);
                        } else {
                            b = mul(mesh_scale, vertex3);
                            c = mul(mesh_scale, vertex2);
                        }

                        let result = fcn(a, b, c, triangle_index);
                        if !result {
                            return;
                        }
                    }
                }
            } else {
                // Recurse
                b3_assert!(count <= MESH_STACK_SIZE - 1);
                stack[count] = right_child(node_index, node);
                count += 1;
                node_index = left_child(node_index);

                continue;
            }
        }

        if count == 0 {
            break;
        }
        count -= 1;
        node_index = stack[count];
    }
}
