// Port of box3d/src/compound.c + compound.h
//
// Representation deviations (see PORTING.md): the C b3CompoundData is a single
// allocation with trailing arrays (tree nodes, materials, instance arrays, and
// packed shared hull/mesh blobs) accessed through byte offsets. The Rust
// CompoundData (types.rs) uses Vec fields and Arc-shared hull/mesh data, so the
// offset fixup machinery disappears. Sharing is preserved: identical hulls or
// meshes referenced by several instances resolve to one shared Arc, and
// shared_hull_count/shared_mesh_count keep their diagnostic meaning.
// b3ConvertCompoundToBytes/b3ConvertBytesToCompound (in-place serialization)
// are not ported.

use std::sync::Arc;

use crate::b3_assert;
use crate::capsule::{
    collide_mover_and_capsule, compute_capsule_aabb, overlap_capsule, ray_cast_capsule,
    shape_cast_capsule,
};
use crate::constants::{MAX_CHILD_SHAPES, MAX_SHAPE_CAST_POINTS};
use crate::dynamic_tree::{
    dynamic_tree_box_cast, dynamic_tree_create, dynamic_tree_create_proxy, dynamic_tree_query,
    dynamic_tree_ray_cast, dynamic_tree_rebuild,
};
use crate::math_functions::{
    aabb_transform, add, inv_rotate_vector, inv_transform_point, invert_transform, make_aabb,
    make_matrix_from_quat, max, max_int, min, min_int, mul_mv, mul_transforms, rotate_vector, sub,
    transform_point, vec3, Transform, Vec3, AABB,
};
use crate::sphere::{
    collide_mover_and_sphere, compute_sphere_aabb, overlap_sphere, ray_cast_sphere,
    shape_cast_sphere,
};
use crate::types::{
    BoxCastInput, Capsule, CastOutput, ChildShape, ChildShapeGeom, CompoundCapsule, CompoundData,
    CompoundDef, CompoundHull, CompoundMesh, CompoundSphere, HullData, Mesh, MeshData, PlaneResult,
    RayCastInput, ShapeCastInput, ShapeProxy, SurfaceMaterial, Sweep, DEFAULT_MASK_BITS,
    MAX_COMPOUND_MESH_MATERIALS,
};

const _: () = assert!(MAX_COMPOUND_MESH_MATERIALS == 4, "too many materials in compound mesh");

pub fn get_compound_materials(compound: &CompoundData) -> &[SurfaceMaterial] {
    &compound.materials
}

pub fn get_compound_capsule(compound: &CompoundData, index: i32) -> CompoundCapsule {
    b3_assert!(0 <= index && index < compound.capsules.len() as i32);
    compound.capsules[index as usize]
}

pub fn get_compound_hull(compound: &CompoundData, index: i32) -> CompoundHull {
    b3_assert!(0 <= index && index < compound.hulls.len() as i32);
    compound.hulls[index as usize].clone()
}

pub fn get_compound_mesh(compound: &CompoundData, index: i32) -> CompoundMesh {
    b3_assert!(0 <= index && index < compound.meshes.len() as i32);
    compound.meshes[index as usize].clone()
}

pub fn get_compound_sphere(compound: &CompoundData, index: i32) -> CompoundSphere {
    b3_assert!(0 <= index && index < compound.spheres.len() as i32);
    compound.spheres[index as usize]
}

/// Children are indexed capsules first, then hulls, then meshes, then spheres
/// (the order they are added to the tree in create_compound).
pub fn get_compound_child(compound: &CompoundData, child_index: i32) -> ChildShape {
    let mut child_index = child_index;

    // Capsule?
    if 0 <= child_index && child_index < compound.capsules.len() as i32 {
        let compound_capsule = get_compound_capsule(compound, child_index);
        return ChildShape {
            geom: ChildShapeGeom::Capsule(compound_capsule.capsule),
            transform: Transform::IDENTITY,
            material_indices: [compound_capsule.material_index, 0, 0, 0],
        };
    }
    child_index -= compound.capsules.len() as i32;

    // Hull?
    if 0 <= child_index && child_index < compound.hulls.len() as i32 {
        let compound_hull = get_compound_hull(compound, child_index);
        return ChildShape {
            geom: ChildShapeGeom::Hull(compound_hull.hull),
            transform: compound_hull.transform,
            material_indices: [compound_hull.material_index, 0, 0, 0],
        };
    }
    child_index -= compound.hulls.len() as i32;

    // Mesh?
    if 0 <= child_index && child_index < compound.meshes.len() as i32 {
        let compound_mesh = get_compound_mesh(compound, child_index);
        let m = compound_mesh.material_indices;

        return ChildShape {
            geom: ChildShapeGeom::Mesh(Mesh {
                data: compound_mesh.mesh_data,
                scale: compound_mesh.scale,
            }),
            transform: compound_mesh.transform,
            material_indices: [m[0], m[1], m[2], m[3]],
        };
    }
    child_index -= compound.meshes.len() as i32;

    b3_assert!(0 <= child_index && child_index < compound.spheres.len() as i32);

    // Sphere
    let compound_sphere = get_compound_sphere(compound, child_index);
    ChildShape {
        geom: ChildShapeGeom::Sphere(compound_sphere.sphere),
        transform: Transform::IDENTITY,
        material_indices: [compound_sphere.material_index, 0, 0, 0],
    }
}

/// Content equality for shared-hull dedup. The C version memcmps the whole hull
/// blob; comparing the geometry arrays is equivalent because the scalar
/// metadata (aabb, volume, inertia, ...) is derived from them by the same code.
fn hull_data_equal(a: &HullData, b: &HullData) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }

    if a.vertices.len() != b.vertices.len()
        || a.points.len() != b.points.len()
        || a.edges.len() != b.edges.len()
        || a.faces.len() != b.faces.len()
        || a.planes.len() != b.planes.len()
    {
        return false;
    }

    if a.points != b.points {
        return false;
    }

    for (va, vb) in a.vertices.iter().zip(&b.vertices) {
        if va.edge != vb.edge {
            return false;
        }
    }

    for (ea, eb) in a.edges.iter().zip(&b.edges) {
        if ea.next != eb.next || ea.twin != eb.twin || ea.origin != eb.origin || ea.face != eb.face
        {
            return false;
        }
    }

    for (fa, fb) in a.faces.iter().zip(&b.faces) {
        if fa.edge != fb.edge {
            return false;
        }
    }

    if a.planes != b.planes {
        return false;
    }

    true
}

/// Content equality for shared-mesh dedup (C: byteCount + memcmp over the blob).
/// The BVH nodes are derived from the triangles by the same code, so comparing
/// vertices/triangles/materials/flags is equivalent.
fn mesh_data_equal(a: &MeshData, b: &MeshData) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }

    if a.vertices.len() != b.vertices.len()
        || a.triangles.len() != b.triangles.len()
        || a.materials.len() != b.materials.len()
        || a.flags.len() != b.flags.len()
    {
        return false;
    }

    if a.vertices != b.vertices {
        return false;
    }

    for (ta, tb) in a.triangles.iter().zip(&b.triangles) {
        if ta.index1 != tb.index1 || ta.index2 != tb.index2 || ta.index3 != tb.index3 {
            return false;
        }
    }

    if a.materials != b.materials {
        return false;
    }

    if a.flags != b.flags {
        return false;
    }

    true
}

/// The C material map (wyhash + memcmp, get_or_insert with value = insertion
/// index). A linear scan produces exactly the same insertion-order indices.
fn find_or_add_material(materials: &mut Vec<SurfaceMaterial>, material: &SurfaceMaterial) -> i32 {
    for (i, m) in materials.iter().enumerate() {
        if m == material {
            return i as i32;
        }
    }

    materials.push(*material);
    materials.len() as i32 - 1
}

/// Mirrors mesh.c: the distinct material count is max(materialIndices) + 1, at
/// least 1. Used for the def/material-count consistency assert.
fn mesh_distinct_material_count(mesh: &MeshData) -> i32 {
    let mut material_count = 1;
    for &index in &mesh.materials {
        material_count = max_int(material_count, index as i32 + 1);
    }
    material_count
}

pub fn create_compound(def: &CompoundDef) -> Arc<CompoundData> {
    let capsule_count = def.capsules.len() as i32;
    let hull_count = def.hulls.len() as i32;
    let mesh_count = def.meshes.len() as i32;
    let sphere_count = def.spheres.len() as i32;

    let convex_count = capsule_count + hull_count + sphere_count;
    let shape_count = convex_count + mesh_count;

    b3_assert!(shape_count < MAX_CHILD_SHAPES);

    let mut tree = dynamic_tree_create(shape_count);

    let mut child_index: i32 = 0;

    // Material map for convex material sharing. Mesh materials are not shared for simplicity.
    let mut materials: Vec<SurfaceMaterial> = Vec::new();

    // Capsules
    let mut capsules: Vec<CompoundCapsule> = Vec::with_capacity(capsule_count as usize);
    for capsule_def in &def.capsules {
        // Look for an existing material, get the shared material index
        let material_index = find_or_add_material(&mut materials, &capsule_def.material);

        capsules.push(CompoundCapsule { capsule: capsule_def.capsule, material_index });

        let aabb = compute_capsule_aabb(&capsule_def.capsule, Transform::IDENTITY);
        dynamic_tree_create_proxy(&mut tree, aabb, !0u64, child_index as u64);
        child_index += 1;
    }

    // Hulls
    let mut shared_hulls: Vec<Arc<HullData>> = Vec::new();
    let mut hulls: Vec<CompoundHull> = Vec::with_capacity(hull_count as usize);
    for hull_def in &def.hulls {
        let aabb = crate::hull::compute_hull_aabb(&hull_def.hull, hull_def.transform);
        dynamic_tree_create_proxy(&mut tree, aabb, !0u64, child_index as u64);
        child_index += 1;

        // Look for an existing material
        let material_index = find_or_add_material(&mut materials, &hull_def.material);

        // Look for an existing matching hull
        let shared = match shared_hulls
            .iter()
            .find(|h| Arc::ptr_eq(h, &hull_def.hull) || hull_data_equal(h, &hull_def.hull))
        {
            Some(h) => h.clone(),
            None => {
                // This is a new shared hull
                shared_hulls.push(hull_def.hull.clone());
                hull_def.hull.clone()
            }
        };

        hulls.push(CompoundHull { hull: shared, transform: hull_def.transform, material_index });
    }

    // Meshes
    let mut shared_meshes: Vec<Arc<MeshData>> = Vec::new();
    let mut meshes: Vec<CompoundMesh> = Vec::with_capacity(mesh_count as usize);
    for mesh_def in &def.meshes {
        let mesh_data = &mesh_def.mesh_data;
        let aabb = crate::mesh::compute_mesh_aabb(mesh_data, mesh_def.transform, mesh_def.scale);
        dynamic_tree_create_proxy(&mut tree, aabb, !0u64, child_index as u64);
        child_index += 1;

        // No effort to share mesh materials. It would be easier to do if the
        // number of materials was limited.
        b3_assert!(!mesh_def.materials.is_empty());
        b3_assert!(mesh_distinct_material_count(mesh_data) == mesh_def.materials.len() as i32);
        b3_assert!(mesh_def.materials.len() <= MAX_COMPOUND_MESH_MATERIALS);

        let mut material_indices = [0i32; MAX_COMPOUND_MESH_MATERIALS];
        for (j, material) in mesh_def.materials.iter().enumerate() {
            // Look for an existing material
            material_indices[j] = find_or_add_material(&mut materials, material);
        }

        // Look for an existing matching mesh
        let shared = match shared_meshes
            .iter()
            .find(|m| Arc::ptr_eq(m, mesh_data) || mesh_data_equal(m, mesh_data))
        {
            Some(m) => m.clone(),
            None => {
                // This is a new shared mesh
                shared_meshes.push(mesh_data.clone());
                mesh_data.clone()
            }
        };

        meshes.push(CompoundMesh {
            mesh_data: shared,
            transform: mesh_def.transform,
            scale: mesh_def.scale,
            material_indices,
        });
    }

    // Spheres
    let mut spheres: Vec<CompoundSphere> = Vec::with_capacity(sphere_count as usize);
    for sphere_def in &def.spheres {
        // Look for an existing material
        let material_index = find_or_add_material(&mut materials, &sphere_def.material);

        spheres.push(CompoundSphere { sphere: sphere_def.sphere, material_index });

        let aabb = compute_sphere_aabb(&sphere_def.sphere, Transform::IDENTITY);
        dynamic_tree_create_proxy(&mut tree, aabb, !0u64, child_index as u64);
        child_index += 1;
    }

    b3_assert!(!materials.is_empty());
    b3_assert!(tree.node_count > 0);

    dynamic_tree_rebuild(&mut tree, true);

    // The C code embeds the tree nodes in the compound blob and scrubs the
    // free list and rebuild scratch arrays; do the same on the owned tree.
    tree.free_list = 0;
    tree.leaf_indices = Vec::new();
    tree.leaf_boxes = Vec::new();
    tree.leaf_centers = Vec::new();
    tree.bin_indices = Vec::new();
    tree.rebuild_capacity = 0;

    Arc::new(CompoundData {
        tree,
        materials,
        capsules,
        hulls,
        shared_hull_count: shared_hulls.len() as i32,
        meshes,
        shared_mesh_count: shared_meshes.len() as i32,
        spheres,
    })
}

pub fn compute_compound_aabb(shape: &CompoundData, transform: Transform) -> AABB {
    b3_assert!(!shape.tree.nodes.is_empty());

    let root = shape.tree.root;
    let aabb = shape.tree.nodes[root as usize].aabb;
    aabb_transform(transform, aabb)
}

pub fn overlap_compound(shape: &CompoundData, shape_transform: Transform, proxy: &ShapeProxy) -> bool {
    let mut overlap = false;

    let mut aabb = AABB { lower_bound: proxy.points[0], upper_bound: proxy.points[0] };
    for i in 1..proxy.count() {
        aabb.lower_bound = min(aabb.lower_bound, proxy.points[i as usize]);
        aabb.upper_bound = max(aabb.upper_bound, proxy.points[i as usize]);
    }

    let r = vec3(proxy.radius, proxy.radius, proxy.radius);
    aabb.lower_bound = sub(aabb.lower_bound, r);
    aabb.upper_bound = add(aabb.upper_bound, r);

    let _ = dynamic_tree_query(
        &shape.tree,
        aabb,
        !0u64,
        false,
        &mut |_proxy_id: i32, user_data: u64| -> bool {
            let child_index = user_data as i32;
            let child = get_compound_child(shape, child_index);

            let transform = mul_transforms(shape_transform, child.transform);

            let child_overlap = match &child.geom {
                ChildShapeGeom::Capsule(capsule) => overlap_capsule(capsule, transform, proxy),
                ChildShapeGeom::Hull(hull) => crate::hull::overlap_hull(hull, transform, proxy),
                ChildShapeGeom::Mesh(mesh) => crate::mesh::overlap_mesh(mesh, transform, proxy),
                ChildShapeGeom::Sphere(sphere) => overlap_sphere(sphere, transform, proxy),
            };

            if child_overlap {
                // Done
                overlap = true;
                return false;
            }

            // Continue the query if there is no overlap
            true
        },
    );

    overlap
}

pub fn ray_cast_compound(shape: &CompoundData, input: &RayCastInput) -> CastOutput {
    let mut result = CastOutput::default();

    let _ = dynamic_tree_ray_cast(
        &shape.tree,
        input,
        !0u64,
        false,
        &mut |input: &RayCastInput, _proxy_id: i32, user_data: u64| -> f32 {
            let child_index = user_data as i32;

            let child = get_compound_child(shape, child_index);

            let mut local_input = *input;
            local_input.origin = inv_transform_point(child.transform, input.origin);
            local_input.translation = inv_rotate_vector(child.transform.q, input.translation);

            let mut output = match &child.geom {
                ChildShapeGeom::Capsule(capsule) => {
                    let mut output = ray_cast_capsule(capsule, &local_input);
                    output.material_index = child.material_indices[0];
                    output
                }
                ChildShapeGeom::Hull(hull) => {
                    let mut output = crate::hull::ray_cast_hull(hull, &local_input);
                    output.material_index = child.material_indices[0];
                    output
                }
                ChildShapeGeom::Mesh(mesh) => {
                    let mut output = crate::mesh::ray_cast_mesh(mesh, &local_input);
                    b3_assert!(0 <= output.material_index);
                    let child_material_index =
                        min_int(output.material_index, MAX_COMPOUND_MESH_MATERIALS as i32 - 1);
                    output.material_index = child.material_indices[child_material_index as usize];
                    output
                }
                ChildShapeGeom::Sphere(sphere) => {
                    let mut output = ray_cast_sphere(sphere, &local_input);
                    output.material_index = child.material_indices[0];
                    output
                }
            };

            if output.hit {
                output.point = transform_point(child.transform, output.point);
                output.normal = rotate_vector(child.transform.q, output.normal);
                output.child_index = child_index;
                result = output;
                return output.fraction;
            }

            input.max_fraction
        },
    );

    result
}

pub fn shape_cast_compound(shape: &CompoundData, input: &ShapeCastInput) -> CastOutput {
    let mut result = CastOutput::default();

    if input.proxy.count() == 0 {
        return result;
    }

    let shape_input = input;

    // The compound tree is in the compound local frame, so the proxy box needs no origin offset
    let box_ = make_aabb(shape_input.proxy.points, shape_input.proxy.radius);
    let tree_input = BoxCastInput {
        box_,
        translation: shape_input.translation,
        max_fraction: shape_input.max_fraction,
    };

    let _ = dynamic_tree_box_cast(
        &shape.tree,
        &tree_input,
        !0u64,
        false,
        &mut |input: &BoxCastInput, _proxy_id: i32, user_data: u64| -> f32 {
            let child_index = user_data as i32;

            let child = get_compound_child(shape, child_index);

            // Rebuild from the carried shape cast input, taking only the
            // advancing fraction from the tree
            let count = min_int(shape_input.proxy.count(), MAX_SHAPE_CAST_POINTS as i32);
            let mut local_points = [Vec3::ZERO; MAX_SHAPE_CAST_POINTS];

            let inv_transform = invert_transform(child.transform);
            let r = make_matrix_from_quat(inv_transform.q);

            for i in 0..count {
                local_points[i as usize] =
                    add(mul_mv(r, shape_input.proxy.points[i as usize]), inv_transform.p);
            }

            let local_input = ShapeCastInput {
                proxy: ShapeProxy {
                    points: &local_points[..count as usize],
                    radius: shape_input.proxy.radius,
                },
                translation: mul_mv(r, shape_input.translation),
                max_fraction: input.max_fraction,
                can_encroach: shape_input.can_encroach,
            };

            let mut output = match &child.geom {
                ChildShapeGeom::Capsule(capsule) => {
                    let mut output = shape_cast_capsule(capsule, &local_input);
                    output.material_index = child.material_indices[0];
                    output
                }
                ChildShapeGeom::Hull(hull) => {
                    let mut output = crate::hull::shape_cast_hull(hull, &local_input);
                    output.material_index = child.material_indices[0];
                    output
                }
                ChildShapeGeom::Mesh(mesh) => {
                    let mut output = crate::mesh::shape_cast_mesh(mesh, &local_input);
                    b3_assert!(0 <= output.material_index);
                    let child_material_index =
                        min_int(output.material_index, MAX_COMPOUND_MESH_MATERIALS as i32 - 1);
                    output.material_index = child.material_indices[child_material_index as usize];
                    output
                }
                ChildShapeGeom::Sphere(sphere) => {
                    let mut output = shape_cast_sphere(sphere, &local_input);
                    output.material_index = child.material_indices[0];
                    output
                }
            };

            if output.hit {
                output.point = transform_point(child.transform, output.point);
                output.normal = rotate_vector(child.transform.q, output.normal);
                output.child_index = child_index;
                result = output;
                return output.fraction;
            }

            input.max_fraction
        },
    );

    result
}

/// C: b3QueryCompound with b3CompoundQueryFcn + context; the closure captures
/// the context.
pub fn query_compound(
    compound: &CompoundData,
    aabb: AABB,
    fcn: &mut dyn FnMut(&CompoundData, i32) -> bool,
) {
    let _ = dynamic_tree_query(
        &compound.tree,
        aabb,
        DEFAULT_MASK_BITS,
        false,
        &mut |_proxy_id: i32, user_data: u64| -> bool { fcn(compound, user_data as i32) },
    );
}

/// Transforms a sweep for a compound child shape.
/// xf = xfP * xfC
pub fn make_compound_child_sweep(compound_transform: Transform, child_transform: Transform) -> Sweep {
    let xf = mul_transforms(compound_transform, child_transform);
    Sweep {
        local_center: Vec3::ZERO,
        c1: xf.p,
        c2: xf.p,
        q1: xf.q,
        q2: xf.q,
    }
}

/// C: b3CollideMoverAndCompound(planes, capacity, shape, mover) — the capacity
/// is planes.len().
pub fn collide_mover_and_compound(planes: &mut [PlaneResult], shape: &CompoundData, mover: &Capsule) -> i32 {
    let plane_capacity = planes.len() as i32;
    let mut plane_count: i32 = 0;

    let mut aabb = AABB {
        lower_bound: min(mover.center1, mover.center2),
        upper_bound: max(mover.center1, mover.center2),
    };
    let r = vec3(mover.radius, mover.radius, mover.radius);
    aabb.lower_bound = sub(aabb.lower_bound, r);
    aabb.upper_bound = add(aabb.upper_bound, r);

    let _ = dynamic_tree_query(
        &shape.tree,
        aabb,
        !0u64,
        false,
        &mut |_proxy_id: i32, user_data: u64| -> bool {
            let child_index = user_data as i32;
            let child = get_compound_child(shape, child_index);

            // Transform mover to child space
            let local_mover = Capsule {
                center1: inv_transform_point(child.transform, mover.center1),
                center2: inv_transform_point(child.transform, mover.center2),
                radius: mover.radius,
            };

            let capacity = plane_capacity - plane_count;
            b3_assert!(capacity > 0);

            let child_planes = &mut planes[plane_count as usize..];
            let count = match &child.geom {
                ChildShapeGeom::Capsule(capsule) => {
                    collide_mover_and_capsule(&mut child_planes[0], capsule, &local_mover)
                }
                ChildShapeGeom::Hull(hull) => {
                    crate::hull::collide_mover_and_hull(&mut child_planes[0], hull, &local_mover)
                }
                ChildShapeGeom::Mesh(mesh) => {
                    crate::mesh::collide_mover_and_mesh(child_planes, mesh, &local_mover)
                }
                ChildShapeGeom::Sphere(sphere) => {
                    collide_mover_and_sphere(&mut child_planes[0], sphere, &local_mover)
                }
            };

            // Transform results back to shape space
            for plane in child_planes[..count as usize].iter_mut() {
                plane.plane.normal = rotate_vector(child.transform.q, plane.plane.normal);
                plane.point = transform_point(child.transform, plane.point);
            }

            plane_count += count;

            // Continue query while there is room for more planes
            plane_count < plane_capacity
        },
    );

    plane_count
}
