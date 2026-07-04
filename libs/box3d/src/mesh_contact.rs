// Port of box3d/src/mesh_contact.c
//
// Deviations:
// - Arena allocations (manifold buffer, point buffer, clusters, old manifold
//   copy) are local Vecs. The C shared point buffer becomes a per-manifold
//   Vec<LocalManifoldPoint>; the total point budget and per-triangle capacity
//   bookkeeping are preserved exactly.
// - The C b3LocalManifold** pointer arrays become index arrays into the
//   manifold buffer.
// - The C QSORT (qsort.h) of tentative triangles becomes sort_unstable_by with
//   the same key; the order of exactly-equal keys may differ from C.
// - The "complex mesh" warning once-flag is an AtomicBool.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::{linear_slop, max_aabb_margin, mesh_rest_offset, speculative_distance, MAX_MANIFOLD_POINTS};
use crate::contact::{Contact, ContactCache, MeshContact, TriangleCache, FORCE_GHOST_COLLISIONS};
use crate::core::NULL_INDEX;
use crate::manifold::make_feature_id;
use crate::math_functions::{
    aabb_contains, aabb_transform, abs_float, add, clamp_int, cross, dot, invert_transform, is_valid_float,
    make_matrix_from_quat, max_float, max_int, min_float, min_int, mul_mv, mul_sv, perp, rotate_vector, sub, sub_pos,
    to_relative_transform, vec2, Vec2, Vec3, WorldTransform, AABB, POS_ZERO,
};
use crate::math_internal::{cross2, distance_squared2, make_normal_from_points, sub2, Triangle};
use crate::physics_world::World;
use crate::shape::{get_shape_materials, Shape, ShapeGeometry};
use crate::types::{
    LocalManifold, LocalManifoldPoint, Manifold, SATCache, SeparatingFeature, ShapeType, TriangleFeature,
    ALL_FLAT_EDGES, FLAT_EDGE1, FLAT_EDGE2, FLAT_EDGE3,
};

// This guards against excessive memory usage and complex collision
const MAX_MESH_CONTACT_TRIANGLES: usize = 256;
const MAX_POINTS_PER_TRIANGLE: i32 = 32;

fn is_sorted(array: &[i32]) -> bool {
    for i in 0..array.len().saturating_sub(1) {
        if array[i] >= array[i + 1] {
            return false;
        }
    }

    true
}

fn query_mesh_triangles(indices: &mut [i32], capacity: i32, mesh: &crate::types::Mesh, bounds: AABB) -> i32 {
    let mut count = 0i32;
    crate::mesh::query_mesh(mesh, bounds, &mut |_a, _b, _c, triangle_index| {
        if count == capacity {
            return false;
        }
        indices[count as usize] = triangle_index;
        count += 1;
        count < capacity
    });
    count
}

fn query_height_field_triangles(
    indices: &mut [i32],
    capacity: i32,
    height_field: &crate::types::HeightFieldData,
    bounds: AABB,
) -> i32 {
    let mut count = 0i32;
    crate::height_field::query_height_field(height_field, bounds, &mut |_a, _b, _c, triangle_index| {
        if count == capacity {
            return false;
        }
        indices[count as usize] = triangle_index;
        count += 1;
        count < capacity
    });
    count
}

static COMPLEX_MESH_WARNING: AtomicBool = AtomicBool::new(false);

fn refresh_cache(mesh_contact: &mut MeshContact, shape_a: &Shape, xf_a: WorldTransform, bounds: &AABB) {
    b3_assert!(shape_a.shape_type() == ShapeType::Mesh || shape_a.shape_type() == ShapeType::Height);

    // If the dynamic body didn't move out of the cached query bounds we are done!
    if aabb_contains(mesh_contact.query_bounds, *bounds) {
        if let ShapeGeometry::Mesh(mesh) = &shape_a.geom {
            for cache in &mesh_contact.triangle_cache {
                b3_assert!(0 <= cache.triangle_index && cache.triangle_index < mesh.data.triangle_count());
                let _ = cache;
            }
            let _ = mesh;
        }

        return;
    }

    // Enlarge to the query bounds to absorb small movement
    let radius = max_aabb_margin() + speculative_distance();
    let extension = Vec3 { x: radius, y: radius, z: radius };
    mesh_contact.query_bounds.lower_bound = sub(bounds.lower_bound, extension);
    mesh_contact.query_bounds.upper_bound = add(bounds.upper_bound, extension);

    // Query triangles
    let triangle_capacity = MAX_MESH_CONTACT_TRIANGLES as i32;

    let mut triangle_indices = [0i32; MAX_MESH_CONTACT_TRIANGLES];

    // Bounds are in world space. Convert to the local mesh frame. The broadphase bounds are float,
    // so the demoted mesh transform is the matching float world frame (exact in float mode).
    let mesh_transform = to_relative_transform(xf_a, POS_ZERO);
    let local_bounds = aabb_transform(invert_transform(mesh_transform), mesh_contact.query_bounds);
    let triangle_count = match &shape_a.geom {
        ShapeGeometry::Mesh(mesh) => query_mesh_triangles(&mut triangle_indices, triangle_capacity, mesh, local_bounds),
        ShapeGeometry::HeightField(height_field) => {
            query_height_field_triangles(&mut triangle_indices, triangle_capacity, height_field, local_bounds)
        }
        _ => {
            b3_assert!(false);
            0
        }
    };

    if triangle_count == triangle_capacity && !COMPLEX_MESH_WARNING.swap(true, Ordering::Relaxed) {
        crate::core::log(&format!(
            "WARNING: complex mesh detected, triangle buffer capacity of {} reached",
            triangle_capacity
        ));
    }

    // Triangle indices must be sorted to match caches.
    b3_validate!(is_sorted(&triangle_indices[..triangle_count as usize]));

    // Create new contact cache and match with old one
    let mut contact_cache = [ContactCache::default(); MAX_MESH_CONTACT_TRIANGLES];

    let mut index2 = 0usize;
    for index1 in 0..triangle_count as usize {
        contact_cache[index1] = ContactCache::default();

        while index2 < mesh_contact.triangle_cache.len()
            && mesh_contact.triangle_cache[index2].triangle_index < triangle_indices[index1]
        {
            index2 += 1;
        }

        if index2 < mesh_contact.triangle_cache.len()
            && mesh_contact.triangle_cache[index2].triangle_index == triangle_indices[index1]
        {
            contact_cache[index1] = mesh_contact.triangle_cache[index2].cache;
        }
    }

    // Save new cache
    mesh_contact.triangle_cache.resize(triangle_count as usize, TriangleCache::default());
    for i in 0..triangle_count as usize {
        mesh_contact.triangle_cache[i] = TriangleCache {
            triangle_index: triangle_indices[i],
            cache: contact_cache[i],
        };

        if let ShapeGeometry::Mesh(mesh) = &shape_a.geom {
            b3_assert!(
                0 <= mesh_contact.triangle_cache[i].triangle_index
                    && mesh_contact.triangle_cache[i].triangle_index < mesh.data.triangle_count()
            );
            let _ = mesh;
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TentativeTriangle {
    squared_distance: f32,
    index: i32,
}

const MAX_EDGE_COUNT: usize = 64;

struct FoundEdges {
    keys: [u64; MAX_EDGE_COUNT],
    count: i32,
}

#[inline]
fn add_edge(edges: &mut FoundEdges, vertex1: i32, vertex2: i32) -> bool {
    let i1 = min_int(vertex1, vertex2) as u64;
    let i2 = max_int(vertex1, vertex2) as u64;
    let key = i1 << 32 | i2;

    let count = edges.count;
    for i in 0..count {
        if edges.keys[i as usize] == key {
            return false;
        }
    }

    if count == MAX_EDGE_COUNT as i32 {
        // This will lead to a potential ghost collision
        return true;
    }

    edges.keys[count as usize] = key;
    edges.count += 1;

    true
}

#[inline]
fn find_edge(edges: &FoundEdges, vertex1: i32, vertex2: i32) -> bool {
    let i1 = min_int(vertex1, vertex2) as u64;
    let i2 = max_int(vertex1, vertex2) as u64;
    let key = i1 << 32 | i2;

    let count = edges.count;
    for i in 0..count {
        if edges.keys[i as usize] == key {
            return true;
        }
    }

    false
}

const MAX_VERTEX_COUNT: usize = 64;

struct FoundVertices {
    keys: [i32; MAX_VERTEX_COUNT],
    count: i32,
}

#[inline]
fn add_vertex(vertices: &mut FoundVertices, vertex: i32) -> bool {
    let key = vertex;

    let count = vertices.count;
    for i in 0..count {
        if vertices.keys[i as usize] == key {
            return false;
        }
    }

    if count == MAX_VERTEX_COUNT as i32 {
        // This will lead to a potential ghost collision
        return true;
    }

    vertices.keys[count as usize] = key;
    vertices.count += 1;

    true
}

// Returns true if (score, separation) should replace (bestScore, bestSeparation).
#[inline]
fn is_better_cull_candidate(
    score: f32,
    separation: f32,
    best_score: f32,
    best_separation: f32,
    score_tol: f32,
    separation_tol: f32,
) -> bool {
    if score > best_score + score_tol {
        return true;
    }
    if score < best_score - score_tol {
        return false;
    }

    // Break the tie using separation
    separation < best_separation - separation_tol
}

#[derive(Clone, Copy, Debug, Default)]
struct Point2D {
    p: Vec2,
    separation: f32,
    original_index: i32,
}

fn cull_points(points: &mut [Point2D]) -> i32 {
    let count = points.len() as i32;
    if count <= 1 {
        return count;
    }

    let tol = 0.25 * linear_slop();
    let tol_sqr = tol * tol;
    let separation_tol = linear_slop();

    let mut final_points = [Point2D::default(); 4];
    let mut count1 = count;

    // Step 1: the two points with the largest distance, ties broken by deepest combined separation
    let mut best_score = 0.0;
    let mut best_separation = f32::MAX;
    let mut best_index1 = NULL_INDEX;
    let mut best_index2 = NULL_INDEX;

    for i in 0..count1 {
        let p1 = points[i as usize].p;
        for j in (i + 1)..count1 {
            let score = distance_squared2(p1, points[j as usize].p);
            // Separation sum heuristic
            let separation = points[i as usize].separation + points[j as usize].separation;

            if is_better_cull_candidate(score, separation, best_score, best_separation, tol_sqr, separation_tol) {
                best_index1 = i;
                best_index2 = j;
                best_score = score;
                best_separation = separation;
            }
        }
    }

    if best_score < tol_sqr {
        // Choose deepest point
        let mut deepest_index = 0i32;
        for i in 1..count1 {
            if points[i as usize].separation < points[deepest_index as usize].separation {
                deepest_index = i;
            }
        }

        if deepest_index != 0 {
            points[0] = points[deepest_index as usize];
        }
        return 1;
    }

    final_points[0] = points[best_index1 as usize];
    final_points[1] = points[best_index2 as usize];

    // Cull
    points[best_index2 as usize] = points[(count1 - 1) as usize];
    points[best_index1 as usize] = points[(count1 - 2) as usize];
    count1 -= 2;

    if count1 == 0 {
        points[0] = final_points[0];
        points[1] = final_points[1];
        return 2;
    }

    // First anchor point
    let a = final_points[0].p;

    // Second anchor point
    let mut b = final_points[1].p;
    let mut ba = sub2(b, a);

    // Step 2: find the point with the maximum triangular area, ties broken by deepest separation
    let mut best_score = 0.0;
    let mut best_separation = f32::MAX;
    let mut best_index = NULL_INDEX;
    let mut best_signed_area = 0.0;
    for i in 0..count1 {
        let p = points[i as usize].p;
        let signed_area = cross2(ba, sub2(p, a));
        let score = abs_float(signed_area);

        if is_better_cull_candidate(score, points[i as usize].separation, best_score, best_separation, tol_sqr, separation_tol) {
            best_signed_area = signed_area;
            best_score = score;
            best_separation = points[i as usize].separation;
            best_index = i;
        }
    }

    if best_index == NULL_INDEX {
        // All points collinear
        points[0] = final_points[0];
        points[1] = final_points[1];
        return 2;
    }

    // Store best point
    final_points[2] = points[best_index as usize];

    if count1 == 1 {
        points[0] = final_points[0];
        points[1] = final_points[1];
        points[2] = final_points[2];
        return 3;
    }

    // Cull
    points[best_index as usize] = points[(count1 - 1) as usize];
    count1 -= 1;

    // Step 4: get the point that adds the most area outside the current triangle

    // Third anchor
    let mut c = final_points[2].p;

    // Ensure CCW ordering
    if best_signed_area < 0.0 {
        std::mem::swap(&mut b, &mut c);
        ba = sub2(b, a);
    }

    let cb = sub2(c, b);
    let ac = sub2(a, c);

    let mut best_score = 0.0;
    let mut best_separation = f32::MAX;
    let mut best_index = NULL_INDEX;
    for i in 0..count1 {
        let p = points[i as usize].p;
        let u1 = cross2(sub2(p, a), ba);
        let u2 = cross2(sub2(p, b), cb);
        let u3 = cross2(sub2(p, c), ac);
        let score = max_float(u1, max_float(u2, u3));

        // Use the area tolerance for collinear points and hysteresis
        if is_better_cull_candidate(score, points[i as usize].separation, best_score, best_separation, tol_sqr, separation_tol) {
            best_score = score;
            best_separation = points[i as usize].separation;
            best_index = i;
        }
    }

    if best_index == NULL_INDEX {
        // No additional area
        points[0] = final_points[0];
        points[1] = final_points[1];
        points[2] = final_points[2];
        return 3;
    }

    // Store best point
    final_points[3] = points[best_index as usize];

    // Full quad
    points[0] = final_points[0];
    points[1] = final_points[1];
    points[2] = final_points[2];
    points[3] = final_points[3];
    4
}

fn reduce_cluster(points: &mut Vec<LocalManifoldPoint>, normal: Vec3) {
    let target_count = 1;
    let count1 = points.len() as i32;
    if count1 <= target_count {
        return;
    }

    let mut pts: Vec<Point2D> = Vec::with_capacity(count1 as usize);
    let u = perp(normal);
    let v = cross(normal, u);
    let origin = points[0].point;

    for i in 0..count1 {
        let d = sub(points[i as usize].point, origin);
        pts.push(Point2D {
            p: vec2(dot(d, u), dot(d, v)),
            separation: points[i as usize].separation,
            original_index: i,
        });
    }

    let count2 = cull_points(&mut pts);
    b3_assert!(count2 <= MAX_MANIFOLD_POINTS as i32);

    let mut final_points = [LocalManifoldPoint::default(); MAX_MANIFOLD_POINTS];
    for i in 0..count2 {
        let index = pts[i as usize].original_index;
        b3_assert!(0 <= index && index < count1);
        final_points[i as usize] = points[index as usize];
    }

    for i in 0..count2 {
        points[i as usize] = final_points[i as usize];
    }
    points.truncate(count2 as usize);
}

struct Cluster {
    manifold_normal: Vec3,
    triangle_normal: Vec3,
    points: Vec<LocalManifoldPoint>,
    point_capacity: i32,
}

/// C: b3ComputeMeshManifolds. The contact's MeshContact is moved out of the
/// world for the duration of the computation so the world stays borrowable.
#[allow(clippy::too_many_arguments)]
pub fn compute_mesh_manifolds(
    world: &World,
    task_context: &mut crate::physics_world::TaskContext,
    contact: &mut Contact,
    shape_a: &Shape,
    material_map: Option<&[i32]>,
    xf_a: WorldTransform,
    shape_b: &Shape,
    xf_b: WorldTransform,
    is_fast: bool,
) -> bool {
    let mut mesh_contact = std::mem::take(&mut contact.mesh_contact);
    let touching = compute_mesh_manifolds_inner(
        world,
        task_context,
        contact,
        &mut mesh_contact,
        shape_a,
        material_map,
        xf_a,
        shape_b,
        xf_b,
        is_fast,
    );
    contact.mesh_contact = mesh_contact;
    touching
}

#[allow(clippy::too_many_arguments)]
fn compute_mesh_manifolds_inner(
    world: &World,
    task_context: &mut crate::physics_world::TaskContext,
    contact: &mut Contact,
    mesh_contact: &mut MeshContact,
    shape_a: &Shape,
    material_map: Option<&[i32]>,
    xf_a: WorldTransform,
    shape_b: &Shape,
    xf_b: WorldTransform,
    is_fast: bool,
) -> bool {
    b3_assert!(shape_a.shape_type() == ShapeType::Mesh || shape_a.shape_type() == ShapeType::Height);

    refresh_cache(mesh_contact, shape_a, xf_a, &shape_b.aabb);

    // Collide with triangles and build manifolds
    let triangle_count = mesh_contact.triangle_cache.len() as i32;

    // Indices into manifold_buffer (C: b3LocalManifold** pointer arrays).
    let mut accepted_manifolds: Vec<usize> = Vec::with_capacity(triangle_count as usize);
    let mut tentative_manifolds: Vec<usize> = Vec::with_capacity(triangle_count as usize);
    let mut tentative_triangles: Vec<TentativeTriangle> = Vec::with_capacity(triangle_count as usize);

    let mut found_edges = FoundEdges { keys: [0; MAX_EDGE_COUNT], count: 0 };
    let mut found_vertices = FoundVertices { keys: [0; MAX_VERTEX_COUNT], count: 0 };

    // This transform converts from mesh frame into the shapeB frame
    let transform_a_to_b = crate::math_functions::inv_mul_world_transforms(xf_b, xf_a);
    let relative_matrix = make_matrix_from_quat(transform_a_to_b.q);
    let linear_slop = linear_slop();

    // This should push apart shapes after a time of impact event.
    // In the past I've called this `polygon skin`, but PhysX and Unreal
    // call it `rest offset` which seems appropriate in this case.
    // It leads to a small visual gap but seems to improve the quality of mesh
    // collision, especially for hull versus mesh.
    let rest_offset = mesh_rest_offset();

    // Make room for clip points
    let point_buffer_capacity = MAX_POINTS_PER_TRIANGLE * triangle_count;
    let mut total_point_count = 0i32;

    let mut manifold_buffer: Vec<LocalManifold> = Vec::with_capacity(triangle_count as usize);

    for index in 0..triangle_count {
        if total_point_count + 3 >= point_buffer_capacity {
            break;
        }

        let triangle_index = mesh_contact.triangle_cache[index as usize].triangle_index;

        let triangle: Triangle = match &shape_a.geom {
            ShapeGeometry::Mesh(mesh) => crate::mesh::get_mesh_triangle(mesh, triangle_index),
            ShapeGeometry::HeightField(height_field) => {
                crate::height_field::get_height_field_triangle(height_field, triangle_index)
            }
            _ => {
                b3_assert!(false);
                return false;
            }
        };

        // Transform triangle into the shape frame
        let vertices = [
            add(mul_mv(relative_matrix, triangle.vertices[0]), transform_a_to_b.p),
            add(mul_mv(relative_matrix, triangle.vertices[1]), transform_a_to_b.p),
            add(mul_mv(relative_matrix, triangle.vertices[2]), transform_a_to_b.p),
        ];

        // Copy the cache out (C uses a pointer into the triangle cache array).
        let mut cache = mesh_contact.triangle_cache[index as usize].cache;
        let point_capacity = point_buffer_capacity - total_point_count;
        let mut manifold = LocalManifold::default();
        manifold.triangle_flags = triangle.flags;
        manifold.feature = TriangleFeature::None;

        match shape_b.shape_type() {
            ShapeType::Capsule => {
                crate::triangle_manifold::collide_capsule_and_triangle(
                    &mut manifold,
                    point_capacity,
                    shape_b.as_capsule(),
                    &vertices,
                    &mut cache.simplex_cache,
                );
            }

            ShapeType::Hull => {
                // Cached edge contact is dangerous at high speed because the hull can rotate around the edge and tunnel
                // through the triangle.
                if is_fast && cache.sat_cache.type_ == SeparatingFeature::EdgePairAxis as u8 {
                    cache.sat_cache = SATCache::default();
                }

                crate::triangle_manifold::collide_hull_and_triangle(
                    &mut manifold,
                    point_capacity,
                    shape_b.as_hull(),
                    vertices[0],
                    vertices[1],
                    vertices[2],
                    triangle.flags,
                    &mut cache.sat_cache,
                );
                task_context.sat_call_count += 1;
                task_context.sat_cache_hit_count += cache.sat_cache.hit as i32;
            }

            ShapeType::Sphere => {
                crate::triangle_manifold::collide_sphere_and_triangle(
                    &mut manifold,
                    point_capacity,
                    shape_b.as_sphere(),
                    &vertices,
                );
            }

            _ => {
                b3_assert!(false);
                return false;
            }
        }

        // Write the cache back.
        mesh_contact.triangle_cache[index as usize].cache = cache;

        let manifold_point_count = manifold.point_count();

        if manifold_point_count > 0 {
            b3_assert!(manifold.feature != TriangleFeature::None);

            total_point_count += manifold_point_count;
            manifold.triangle_index = triangle_index;
            manifold.triangle_normal = make_normal_from_points(vertices[0], vertices[1], vertices[2]);
            manifold.i1 = triangle.i1;
            manifold.i2 = triangle.i2;
            manifold.i3 = triangle.i3;

            let manifold_slot = manifold_buffer.len();

            if manifold.feature == TriangleFeature::TriangleFace || FORCE_GHOST_COLLISIONS {
                let _ = add_edge(&mut found_edges, manifold.i1, manifold.i2);
                let _ = add_edge(&mut found_edges, manifold.i2, manifold.i3);
                let _ = add_edge(&mut found_edges, manifold.i3, manifold.i1);
                let _ = add_vertex(&mut found_vertices, manifold.i1);
                let _ = add_vertex(&mut found_vertices, manifold.i2);
                let _ = add_vertex(&mut found_vertices, manifold.i3);

                accepted_manifolds.push(manifold_slot);
            } else if manifold.feature == TriangleFeature::HullFace {
                let cos_normal_angle = dot(manifold.triangle_normal, manifold.normal);
                if cos_normal_angle > 0.5 {
                    let _ = add_edge(&mut found_edges, manifold.i1, manifold.i2);
                    let _ = add_edge(&mut found_edges, manifold.i2, manifold.i3);
                    let _ = add_edge(&mut found_edges, manifold.i3, manifold.i1);
                    let _ = add_vertex(&mut found_vertices, manifold.i1);
                    let _ = add_vertex(&mut found_vertices, manifold.i2);
                    let _ = add_vertex(&mut found_vertices, manifold.i3);

                    accepted_manifolds.push(manifold_slot);
                } else {
                    let mut min_separation = manifold.points[0].separation;
                    for i in 1..manifold_point_count {
                        min_separation = min_float(min_separation, manifold.points[i as usize].separation);
                    }

                    if min_separation < -2.0 * linear_slop {
                        // Deep overlap
                        let _ = add_edge(&mut found_edges, manifold.i1, manifold.i2);
                        let _ = add_edge(&mut found_edges, manifold.i2, manifold.i3);
                        let _ = add_edge(&mut found_edges, manifold.i3, manifold.i1);
                        let _ = add_vertex(&mut found_vertices, manifold.i1);
                        let _ = add_vertex(&mut found_vertices, manifold.i2);
                        let _ = add_vertex(&mut found_vertices, manifold.i3);
                        accepted_manifolds.push(manifold_slot);
                    } else {
                        tentative_triangles.push(TentativeTriangle {
                            squared_distance: manifold.squared_distance,
                            index: tentative_manifolds.len() as i32,
                        });
                        tentative_manifolds.push(manifold_slot);
                    }
                }
            } else {
                tentative_triangles.push(TentativeTriangle {
                    squared_distance: manifold.squared_distance,
                    index: tentative_manifolds.len() as i32,
                });
                tentative_manifolds.push(manifold_slot);
            }

            manifold_buffer.push(manifold);
        }
    }

    b3_assert!(accepted_manifolds.len() as i32 <= triangle_count);
    b3_assert!(tentative_manifolds.len() as i32 <= triangle_count);
    b3_assert!(tentative_triangles.len() as i32 <= triangle_count);

    if shape_b.shape_type() == ShapeType::Sphere {
        // Sort triangles so the closest triangles are processed first
        // (C: QSORT from qsort.h with strict < on squaredDistance)
        tentative_triangles.sort_unstable_by(|a, b| a.squared_distance.total_cmp(&b.squared_distance));

        // Add tentative manifolds in sorted order. Avoid adding manifolds that generate ghost collisions.
        for i in 0..tentative_triangles.len() {
            let m_slot = tentative_manifolds[tentative_triangles[i].index as usize];
            let m = &manifold_buffer[m_slot];

            let added_edge1 = add_edge(&mut found_edges, m.i1, m.i2);
            let added_edge2 = add_edge(&mut found_edges, m.i2, m.i3);
            let added_edge3 = add_edge(&mut found_edges, m.i3, m.i1);
            let added_vertex1 = add_vertex(&mut found_vertices, m.i1);
            let added_vertex2 = add_vertex(&mut found_vertices, m.i2);
            let added_vertex3 = add_vertex(&mut found_vertices, m.i3);

            let feature = m.feature;
            let mut should_collide = false;
            match feature {
                TriangleFeature::None | TriangleFeature::TriangleFace | TriangleFeature::HullFace => {
                    b3_assert!(false);
                }

                TriangleFeature::Edge1 => {
                    should_collide = added_edge1;
                }

                TriangleFeature::Edge2 => {
                    should_collide = added_edge2;
                }

                TriangleFeature::Edge3 => {
                    should_collide = added_edge3;
                }

                TriangleFeature::Vertex1 => {
                    should_collide = added_vertex1;
                }

                TriangleFeature::Vertex2 => {
                    should_collide = added_vertex2;
                }

                TriangleFeature::Vertex3 => {
                    should_collide = added_vertex3;
                }
            }

            if should_collide {
                accepted_manifolds.push(m_slot);
            }
        }
    } else {
        // Problem: hull can tunnel if time of impact is at concave edge
        // Example: flat box sliding down a ramp to a flat bottom
        // Solution: only ignore flat edges
        for i in 0..tentative_manifolds.len() {
            let m_slot = tentative_manifolds[i];
            let m = &manifold_buffer[m_slot];
            let triangle_flags = m.triangle_flags;

            if (triangle_flags & ALL_FLAT_EDGES) == ALL_FLAT_EDGES {
                continue;
            }

            if (triangle_flags & FLAT_EDGE1) == FLAT_EDGE1 && find_edge(&found_edges, m.i1, m.i2) {
                continue;
            }

            if (triangle_flags & FLAT_EDGE2) == FLAT_EDGE2 && find_edge(&found_edges, m.i2, m.i3) {
                continue;
            }

            if (triangle_flags & FLAT_EDGE3) == FLAT_EDGE3 && find_edge(&found_edges, m.i3, m.i1) {
                continue;
            }

            accepted_manifolds.push(m_slot);
        }
    }

    b3_assert!(accepted_manifolds.len() as i32 <= triangle_count);

    if accepted_manifolds.is_empty() {
        if contact.manifold_count() > 0 {
            contact.manifolds = Vec::new();
        }
        return false;
    }

    let accepted_manifold_count = accepted_manifolds.len();
    let mut clusters: Vec<Cluster> = Vec::with_capacity(accepted_manifold_count);
    let mut cluster_memberships: Vec<i32> = vec![NULL_INDEX; accepted_manifold_count];

    // Cluster tolerance is tighter than the warm starting manifold matching tolerance. These
    // serve different purposes.
    let cluster_threshold = 0.996;
    let mut cluster_point_count = 0i32;
    for i in 0..accepted_manifold_count {
        cluster_memberships[i] = NULL_INDEX;

        let manifold = &manifold_buffer[accepted_manifolds[i]];
        cluster_point_count += manifold.point_count();

        // Cluster based on the triangle normal and contact normal.
        // The first cluster found is accepted because the tolerance is tight.
        // todo consider requiring the triangles to be connected by an edge.
        // todo consider looking for the best cluster instead of the first one within tolerance
        // This bool is here to allow quick testing with and without clustering.
        let allow_clustering = true;
        let manifold_normal = manifold.normal;
        let triangle_normal = manifold.triangle_normal;
        let mut cluster_index = NULL_INDEX;
        for j in 0..clusters.len() {
            if !allow_clustering {
                break;
            }

            let cos_manifold_angle = dot(clusters[j].manifold_normal, manifold_normal);
            let cos_triangle_angle = dot(clusters[j].triangle_normal, triangle_normal);
            if cos_manifold_angle <= cluster_threshold || cos_triangle_angle <= cluster_threshold {
                continue;
            }

            // Found a cluster
            cluster_index = j as i32;
            break;
        }

        if cluster_index != NULL_INDEX {
            cluster_memberships[i] = cluster_index;
            clusters[cluster_index as usize].point_capacity += manifold.point_count();
        } else {
            cluster_memberships[i] = clusters.len() as i32;
            clusters.push(Cluster {
                manifold_normal,
                triangle_normal,
                points: Vec::new(),
                point_capacity: manifold.point_count(),
            });
        }
    }

    if cluster_point_count == 0 {
        return false;
    }

    // Setup clusters
    for cluster in clusters.iter_mut() {
        cluster.points = Vec::with_capacity(cluster.point_capacity as usize);
    }

    // Populate clusters
    for i in 0..accepted_manifold_count {
        let cluster_index = cluster_memberships[i];
        if cluster_index == NULL_INDEX {
            continue;
        }

        b3_assert!(0 <= cluster_index && (cluster_index as usize) < clusters.len());

        let am = &manifold_buffer[accepted_manifolds[i]];
        let cm = &mut clusters[cluster_index as usize];
        for j in 0..am.point_count() {
            b3_assert!((cm.points.len() as i32) < cm.point_capacity);
            let ap = &am.points[j as usize];

            cm.points.push(LocalManifoldPoint {
                triangle_index: am.triangle_index,
                point: ap.point,
                separation: ap.separation,
                pair: ap.pair,
            });
        }
    }

    // Simplify clusters
    for cm in clusters.iter_mut() {
        b3_assert!(cm.points.len() as i32 == cm.point_capacity);
        let triangle_normal = cm.triangle_normal;
        reduce_cluster(&mut cm.points, triangle_normal);
    }

    let cluster_count = clusters.len();

    // Make a temporary copy of previous manifolds
    let mut old_manifolds = contact.manifolds.clone();
    let old_manifold_count = old_manifolds.len();

    // Resize manifolds if needed. C frees + reallocates zeroed when the count
    // changed and memsets otherwise; both yield zeroed manifolds of cluster_count.
    let mut new_manifolds = vec![Manifold::default(); cluster_count];

    let mut consumed = vec![false; old_manifold_count];

    let matrix_b = make_matrix_from_quat(xf_b.q);
    let offset_a = sub_pos(xf_b.p, xf_a.p);

    let normal_match_tolerance = 0.995;
    for i in 0..cluster_count {
        let cm = &clusters[i];
        let point_count = cm.points.len() as i32;
        b3_assert!(0 < point_count && point_count <= MAX_MANIFOLD_POINTS as i32);

        let manifold = &mut new_manifolds[i];
        manifold.point_count = point_count;
        manifold.normal = mul_mv(matrix_b, cm.manifold_normal);

        let cluster_normal = mul_mv(matrix_b, cm.manifold_normal);
        let mut best_dot = normal_match_tolerance;
        let mut best_index = NULL_INDEX;

        for j in 0..old_manifold_count {
            if consumed[j] {
                continue;
            }

            let d = dot(old_manifolds[j].normal, cluster_normal);
            if d > best_dot {
                best_index = j as i32;
                best_dot = d;
            }
        }

        let matched_manifold = if best_index != NULL_INDEX {
            let matched = &old_manifolds[best_index as usize];
            manifold.friction_impulse = matched.friction_impulse;
            manifold.rolling_impulse = matched.rolling_impulse;
            manifold.twist_impulse = matched.twist_impulse;
            consumed[best_index as usize] = true;
            Some(best_index as usize)
        } else {
            None
        };

        for j in 0..point_count {
            let source = &cm.points[j as usize];
            let target = &mut manifold.points[j as usize];

            // Contact points are computed in frame B
            target.anchor_b = mul_mv(matrix_b, source.point);
            target.anchor_a = add(target.anchor_b, offset_a);
            target.separation = source.separation - rest_offset;
            target.feature_id = make_feature_id(source.pair);
            target.triangle_index = source.triangle_index;

            // Preserve normal impulse if possible
            if let Some(matched_index) = matched_manifold {
                let matched = &mut old_manifolds[matched_index];
                let old_point_count = matched.point_count;
                for k in 0..old_point_count {
                    let old_pt = &mut matched.points[k as usize];

                    if target.feature_id == old_pt.feature_id && target.triangle_index == old_pt.triangle_index {
                        target.normal_impulse = old_pt.normal_impulse;
                        target.persisted = true;

                        // claimed
                        old_pt.triangle_index = NULL_INDEX;
                        break;
                    }
                }
            }
        }
    }

    // Store the new manifolds (C wrote into contact->manifolds in place).
    contact.manifolds = new_manifolds;

    let materials_a = get_shape_materials(shape_a);
    let material_b = get_shape_materials(shape_b)[0];
    let mut tangent_velocity_a = Vec3::ZERO;

    // Update friction and restitution if the mesh has per triangle material
    // (C: shapeA->materialCount > 0, only non-zero for shapes with a heap material array)
    if !shape_a.materials.is_empty() {
        let mut friction = 0.0;
        let mut restitution = 0.0;
        let mut sample_count = 0.0;

        for i in 0..cluster_count {
            let manifold_point_count = contact.manifolds[i].point_count;
            for j in 0..manifold_point_count {
                let triangle_index = contact.manifolds[i].points[j as usize].triangle_index;
                let mut material_index: i32;
                match &shape_a.geom {
                    ShapeGeometry::Mesh(mesh) => {
                        material_index = mesh.data.materials[triangle_index as usize] as i32;

                        if let Some(map) = material_map {
                            material_index = map[material_index as usize];
                        }
                    }
                    _ => {
                        let height_field = shape_a.as_height_field();
                        material_index = height_field.materials[(triangle_index >> 1) as usize] as i32;
                    }
                }

                material_index = clamp_int(material_index, 0, shape_a.materials.len() as i32 - 1);
                let material = materials_a[material_index as usize];
                friction += crate::contact::mix_friction(
                    world,
                    material.friction,
                    material.user_material_id,
                    material_b.friction,
                    material_b.user_material_id,
                );
                restitution += crate::contact::mix_restitution(
                    world,
                    material.restitution,
                    material.user_material_id,
                    material_b.restitution,
                    material_b.user_material_id,
                );

                tangent_velocity_a = add(tangent_velocity_a, material.tangent_velocity);

                sample_count += 1.0;
            }
        }

        if sample_count > 0.0 {
            let inv_count = 1.0 / sample_count;
            contact.friction = inv_count * friction;
            contact.restitution = inv_count * restitution;
            tangent_velocity_a = mul_sv(inv_count, tangent_velocity_a);
        }

        b3_assert!(is_valid_float(contact.friction) && contact.friction >= 0.0);
        b3_assert!(is_valid_float(contact.restitution) && contact.restitution >= 0.0);
    } else {
        // Keep these updated in case the values on the shapes are modified
        let friction = crate::contact::mix_friction(
            world,
            materials_a[0].friction,
            materials_a[0].user_material_id,
            material_b.friction,
            material_b.user_material_id,
        );
        let restitution = crate::contact::mix_restitution(
            world,
            materials_a[0].restitution,
            materials_a[0].user_material_id,
            material_b.restitution,
            material_b.user_material_id,
        );
        contact.friction = friction;
        contact.restitution = restitution;
        tangent_velocity_a = materials_a[0].tangent_velocity;
    }

    tangent_velocity_a = rotate_vector(xf_a.q, tangent_velocity_a);

    let radius_b = match &shape_b.geom {
        ShapeGeometry::Sphere(sphere) => sphere.radius,
        ShapeGeometry::Capsule(capsule) => capsule.radius,
        ShapeGeometry::Hull(hull) => hull.inner_radius,
        _ => 0.0,
    };

    let tangent_velocity_b = rotate_vector(xf_b.q, material_b.tangent_velocity);
    contact.rolling_resistance = material_b.rolling_resistance * radius_b;
    contact.tangent_velocity = sub(tangent_velocity_a, tangent_velocity_b);
    true
}
