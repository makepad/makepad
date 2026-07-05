// Port of box3d/src/shape.h (+ shape.c functions to be ported below the structs).

use std::sync::Arc;

use crate::math_functions::{Vec3, AABB};
use crate::types::{
    Capsule, CompoundData, Filter, HeightFieldData, HullData, Mesh, QueryFilter, ShapeType, Sphere,
    SurfaceMaterial,
};

/// The C b3Shape geometry union.
#[derive(Clone, Debug)]
pub enum ShapeGeometry {
    Capsule(Capsule),
    Sphere(Sphere),
    Hull(Arc<HullData>),
    Mesh(Mesh),
    HeightField(Arc<HeightFieldData>),
    Compound(Arc<CompoundData>),
}

impl Default for ShapeGeometry {
    fn default() -> Self {
        ShapeGeometry::Sphere(Sphere::default())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub id: i32,
    pub body_id: i32,
    pub prev_shape_id: i32,
    pub next_shape_id: i32,
    pub sensor_index: i32,
    pub proxy_key: i32,
    pub density: f32,
    pub explosion_scale: f32,
    pub aabb_margin: f32,

    pub aabb: AABB,
    pub fat_aabb: AABB,
    pub local_centroid: Vec3,

    pub material: SurfaceMaterial,
    /// Multi material meshes and compounds own an array; empty means "use `material`".
    pub materials: Vec<SurfaceMaterial>,

    pub filter: Filter,
    pub user_data: u64,

    pub generation: u16,
    pub enable_sensor_events: bool,
    pub enable_contact_events: bool,
    pub enable_custom_filtering: bool,
    pub enable_hit_events: bool,
    pub enable_pre_solve_events: bool,
    pub enlarged_aabb: bool,

    /// The C union (tag: shape_type()).
    pub geom: ShapeGeometry,
}

impl Shape {
    /// Copy every field except `geom` from `src`, reusing this shape's
    /// heap buffers (`materials` keeps its capacity via clone_from). Used by
    /// the compound collide path to build the temporary child shape the way C
    /// stack-copies it — without deep-cloning the compound geometry that the
    /// caller immediately replaces.
    pub fn copy_non_geom_from(&mut self, src: &Shape) {
        self.id = src.id;
        self.body_id = src.body_id;
        self.prev_shape_id = src.prev_shape_id;
        self.next_shape_id = src.next_shape_id;
        self.sensor_index = src.sensor_index;
        self.proxy_key = src.proxy_key;
        self.density = src.density;
        self.explosion_scale = src.explosion_scale;
        self.aabb_margin = src.aabb_margin;
        self.aabb = src.aabb;
        self.fat_aabb = src.fat_aabb;
        self.local_centroid = src.local_centroid;
        self.material = src.material;
        self.materials.clone_from(&src.materials);
        self.filter = src.filter;
        self.user_data = src.user_data;
        self.generation = src.generation;
        self.enable_sensor_events = src.enable_sensor_events;
        self.enable_contact_events = src.enable_contact_events;
        self.enable_custom_filtering = src.enable_custom_filtering;
        self.enable_hit_events = src.enable_hit_events;
        self.enable_pre_solve_events = src.enable_pre_solve_events;
        self.enlarged_aabb = src.enlarged_aabb;
    }

    /// The C `shape->type` field (the union tag).
    #[inline]
    pub fn shape_type(&self) -> ShapeType {
        match &self.geom {
            ShapeGeometry::Capsule(_) => ShapeType::Capsule,
            ShapeGeometry::Sphere(_) => ShapeType::Sphere,
            ShapeGeometry::Hull(_) => ShapeType::Hull,
            ShapeGeometry::Mesh(_) => ShapeType::Mesh,
            ShapeGeometry::HeightField(_) => ShapeType::Height,
            ShapeGeometry::Compound(_) => ShapeType::Compound,
        }
    }
}

/// A single material shape keeps its material inline. Multi material meshes and
/// compounds own an array. Reach the materials the same way for both.
#[inline]
pub fn get_shape_materials(shape: &Shape) -> &[SurfaceMaterial] {
    if !shape.materials.is_empty() {
        &shape.materials
    } else {
        std::slice::from_ref(&shape.material)
    }
}

#[inline]
pub fn should_shapes_collide(filter_a: Filter, filter_b: Filter) -> bool {
    if filter_a.group_index == filter_b.group_index && filter_a.group_index != 0 {
        return filter_a.group_index > 0;
    }

    (filter_a.mask_bits & filter_b.category_bits) != 0 && (filter_a.category_bits & filter_b.mask_bits) != 0
}

#[inline]
pub fn should_query_collide(shape_filter: Filter, query_filter: QueryFilter) -> bool {
    (shape_filter.category_bits & query_filter.mask_bits) != 0
        && (shape_filter.mask_bits & query_filter.category_bits) != 0
}

// ---------------------------------------------------------------------------
// Port of shape.c
// ---------------------------------------------------------------------------
//
// Deviations:
// - Recording hooks (B3_REC*) are not ported.
// - b3DumpShape (debug file output) is not ported.
// - shape->userShape / debug shape callbacks are not ported.
// - The C shape->materialCount field is shape_material_count(): the inline
//   material always counts as one. C zeroes the count when allocations are
//   destroyed; the Rust port only clears the materials Vec.
// - make_shape_proxy takes a 2-element caller buffer because the capsule proxy
//   cannot borrow center1/center2 as one slice (C relies on field adjacency).
// - In shape_apply_wind, C caches the body sim pointer before waking the body,
//   which can leave it stale when the wake moves the sim between solver sets;
//   the port re-reads the sim after the wake.
// - The CCD stall logging in shape_time_of_impact (b3GetStallThreshold) is not
//   ported; the triangle visit counter is kept.


use crate::b3_assert;
use crate::b3_validate;
use crate::constants::{max_aabb_margin, speculative_distance, AABB_MARGIN_FRACTION, MAX_SHAPE_CAST_POINTS, MAX_SHAPES};
use crate::core::{get_length_units_per_meter, NULL_INDEX, SECRET_COOKIE};
use crate::id::{BodyId, ContactId, ShapeId, WorldId, NULL_SHAPE_ID};
use crate::id_pool::{alloc_id, free_id};
use crate::math_functions::{
    abs, add, clamp_int, cross, dot, get_length_and_normalize, inv_mul_transforms, inv_rotate_vector,
    inv_transform_point, invert_transform, is_valid_float, is_valid_position, is_valid_vec3, length, lerp,
    make_matrix_from_quat, max, max_float, min_float, min_int, mul_add, mul_mv, mul_sub, mul_sv, mul_transforms,
    normalize, offset_pos, rotate_vector, safe_scale, sub, to_relative_transform, transform_point, vec3, Matrix3,
    Transform, WorldTransform, PI, POS_ZERO,
};
use crate::math_internal::ShapeExtent;
use crate::physics_world::{World, AWAKE_SET, DISABLED_SET, FIRST_SLEEPING_SET};
use crate::sensor::{Sensor, Visitor};
use crate::types::{
    BodyType, CastOutput, ContactData, DistanceInput, MassData, PlaneResult, RayCastInput, ShapeCastInput, ShapeDef,
    ShapeProxy, SimplexCache, Sweep, TOIInput, TOIOutput, WorldCastOutput, MAX_COMPOUND_MESH_MATERIALS,
};

impl Shape {
    #[inline]
    pub fn as_sphere(&self) -> &Sphere {
        match &self.geom {
            ShapeGeometry::Sphere(s) => s,
            _ => panic!("shape is not a sphere"),
        }
    }
    #[inline]
    pub fn as_capsule(&self) -> &Capsule {
        match &self.geom {
            ShapeGeometry::Capsule(c) => c,
            _ => panic!("shape is not a capsule"),
        }
    }
    #[inline]
    pub fn as_hull(&self) -> &Arc<HullData> {
        match &self.geom {
            ShapeGeometry::Hull(h) => h,
            _ => panic!("shape is not a hull"),
        }
    }
    #[inline]
    pub fn as_mesh(&self) -> &Mesh {
        match &self.geom {
            ShapeGeometry::Mesh(m) => m,
            _ => panic!("shape is not a mesh"),
        }
    }
    #[inline]
    pub fn as_height_field(&self) -> &Arc<HeightFieldData> {
        match &self.geom {
            ShapeGeometry::HeightField(h) => h,
            _ => panic!("shape is not a height field"),
        }
    }
    #[inline]
    pub fn as_compound(&self) -> &Arc<CompoundData> {
        match &self.geom {
            ShapeGeometry::Compound(c) => c,
            _ => panic!("shape is not a compound"),
        }
    }
}

/// Mutable variant of get_shape_materials.
#[inline]
pub fn get_shape_materials_mut(shape: &mut Shape) -> &mut [SurfaceMaterial] {
    if !shape.materials.is_empty() {
        &mut shape.materials
    } else {
        std::slice::from_mut(&mut shape.material)
    }
}

/// The C shape->materialCount field.
#[inline]
pub fn shape_material_count(shape: &Shape) -> i32 {
    if shape.materials.is_empty() {
        1
    } else {
        shape.materials.len() as i32
    }
}

/// C: static b3GetShape(world, shapeId). Validates and returns the raw index.
pub fn get_shape_full_id(world: &World, shape_id: ShapeId) -> i32 {
    let id = shape_id.index1 - 1;
    let shape = &world.shapes[id as usize];
    b3_assert!(shape.id == id && shape.generation == shape_id.generation);
    id
}

fn compute_shape_margin(shape: &Shape) -> f32 {
    let margin;

    match &shape.geom {
        ShapeGeometry::Sphere(sphere) => {
            margin = sphere.radius;
        }

        ShapeGeometry::Capsule(capsule) => {
            margin = 0.5 * crate::math_functions::distance(capsule.center2, capsule.center1) + capsule.radius;
        }

        ShapeGeometry::Hull(hull) => {
            let points = &hull.points;
            let mut max_extent_sqr = 0.0;
            let count = hull.vertex_count();
            for i in 0..count {
                let dist_sqr = crate::math_functions::distance_squared(points[i as usize], hull.center);
                max_extent_sqr = max_float(max_extent_sqr, dist_sqr);
            }
            margin = max_extent_sqr.sqrt();
        }

        ShapeGeometry::Mesh(_) | ShapeGeometry::HeightField(_) | ShapeGeometry::Compound(_) => {
            // Static-only shapes: broadphase uses speculative distance for static
            // proxies, so the per-shape margin is never consumed in practice.
            // Return the cap so any incidental use is generous.
            return max_aabb_margin();
        }
    }

    min_float(max_aabb_margin(), AABB_MARGIN_FRACTION * margin)
}

fn update_shape_aabbs(shape: &mut Shape, transform: WorldTransform, proxy_type: BodyType) {
    // Compute a bounding box with a speculative margin
    let speculative_distance = speculative_distance();
    let aabb_margin = shape.aabb_margin;

    let aabb = compute_fat_shape_aabb(shape, transform, speculative_distance);
    shape.aabb = aabb;

    // Smaller margin for static bodies. Cannot be zero due to TOI tolerance.
    let margin = if proxy_type == BodyType::Static { speculative_distance } else { aabb_margin };
    let fat_aabb = AABB {
        lower_bound: Vec3 {
            x: aabb.lower_bound.x - margin,
            y: aabb.lower_bound.y - margin,
            z: aabb.lower_bound.z - margin,
        },
        upper_bound: Vec3 {
            x: aabb.upper_bound.x + margin,
            y: aabb.upper_bound.y + margin,
            z: aabb.upper_bound.z + margin,
        },
    };
    shape.fat_aabb = fat_aabb;
}

/// The C geometry `void*` parameter of b3CreateShapeInternal.
pub enum ShapeGeometryInput<'a> {
    Capsule(&'a Capsule),
    Sphere(&'a Sphere),
    Hull(&'a Arc<HullData>),
    Mesh(&'a Arc<crate::types::MeshData>),
    HeightField(&'a Arc<HeightFieldData>),
    Compound(&'a Arc<CompoundData>),
}

impl ShapeGeometryInput<'_> {
    fn shape_type(&self) -> ShapeType {
        match self {
            ShapeGeometryInput::Capsule(_) => ShapeType::Capsule,
            ShapeGeometryInput::Sphere(_) => ShapeType::Sphere,
            ShapeGeometryInput::Hull(_) => ShapeType::Hull,
            ShapeGeometryInput::Mesh(_) => ShapeType::Mesh,
            ShapeGeometryInput::HeightField(_) => ShapeType::Height,
            ShapeGeometryInput::Compound(_) => ShapeType::Compound,
        }
    }
}

/// C: b3CreateShapeInternal. Returns the new shape id or NULL_INDEX on failure.
#[allow(clippy::too_many_arguments)]
fn create_shape_internal(
    world: &mut World,
    body_id: i32,
    body_transform: WorldTransform,
    def: &ShapeDef,
    geometry: &ShapeGeometryInput,
    shape_transform: Transform,
    scale: Vec3,
    have_shape_transform: bool,
) -> i32 {
    let shape_id = alloc_id(&mut world.shape_id_pool);

    if shape_id == world.shapes.len() as i32 {
        world.shapes.push(Shape::default());
        // A freshly pushed slot has no valid id yet; mark it free like C's zero init
        // (C pushes zeroed memory; id 0 is only distinguishable through the id pool).
        world.shapes[shape_id as usize].id = NULL_INDEX;
    } else {
        b3_assert!(world.shapes[shape_id as usize].id == NULL_INDEX);
    }

    // Resolve the geometry union member first (C switch at the top).
    let geom = match geometry {
        ShapeGeometryInput::Capsule(capsule) => ShapeGeometry::Capsule(**capsule),

        ShapeGeometryInput::Compound(compound) => {
            // Compounds must be a static and not a sensor
            b3_assert!(world.bodies[body_id as usize].body_type == BodyType::Static);
            b3_assert!(def.is_sensor == false);
            ShapeGeometry::Compound(Arc::clone(compound))
        }

        ShapeGeometryInput::Sphere(sphere) => ShapeGeometry::Sphere(**sphere),

        ShapeGeometryInput::Hull(hull) => {
            if have_shape_transform {
                // The transform and non-uniform scale are baked into fresh data, then shared.
                match crate::hull::clone_and_transform_hull(hull, shape_transform, scale) {
                    Some(baked) => ShapeGeometry::Hull(add_hull_to_database(world, &baked)),
                    None => {
                        // This can fail to produce a valid hull in extreme cases
                        free_id(&mut world.shape_id_pool, shape_id);
                        world.shapes[shape_id as usize].id = NULL_INDEX;
                        return NULL_INDEX;
                    }
                }
            } else {
                ShapeGeometry::Hull(add_hull_to_database(world, hull))
            }
        }

        ShapeGeometryInput::Mesh(mesh_data) => ShapeGeometry::Mesh(Mesh {
            data: Arc::clone(mesh_data),
            scale: safe_scale(scale),
        }),

        ShapeGeometryInput::HeightField(height_field) => ShapeGeometry::HeightField(Arc::clone(height_field)),
    };

    let (body_head_shape_id, body_set_index, body_type) = {
        let body = &world.bodies[body_id as usize];
        (body.head_shape_id, body.set_index, body.body_type)
    };

    {
        let shape = &mut world.shapes[shape_id as usize];
        shape.geom = geom;
        shape.id = shape_id;
        shape.body_id = body_id;
        shape.density = def.density;
        shape.explosion_scale = def.explosion_scale;
        shape.filter = def.filter;
        shape.user_data = def.user_data;
        shape.enlarged_aabb = false;
        shape.enable_sensor_events = def.enable_sensor_events;
        shape.enable_contact_events = def.enable_contact_events;
        shape.enable_custom_filtering = def.enable_custom_filtering;
        shape.enable_hit_events = def.enable_hit_events;
        shape.enable_pre_solve_events = def.enable_pre_solve_events;
        shape.proxy_key = NULL_INDEX;
        shape.local_centroid = get_shape_centroid(shape);
        shape.aabb_margin = compute_shape_margin(shape);
        shape.aabb = AABB { lower_bound: Vec3::ZERO, upper_bound: Vec3::ZERO };
        shape.fat_aabb = AABB { lower_bound: Vec3::ZERO, upper_bound: Vec3::ZERO };
        shape.generation = shape.generation.wrapping_add(1);

        if let ShapeGeometry::Compound(compound) = &shape.geom {
            // Own a copy of the compound materials so every shape frees its array the
            // same way. Compounds are few, so the copy is cheap.
            shape.materials = compound.materials.clone();
        } else if def.materials.len() > 1 {
            // Per triangle materials need a heap array.
            shape.materials = def.materials.clone();
        } else {
            // The common case is one material, stored inline with no allocation.
            shape.material = if def.materials.len() == 1 { def.materials[0] } else { def.base_material };
            shape.materials = Vec::new();
        }
    }

    if body_set_index != DISABLED_SET {
        let proxy_type = body_type;
        let force_pair_creation =
            def.invoke_contact_creation && world.shapes[shape_id as usize].shape_type() != ShapeType::Compound;
        create_shape_proxy(world, shape_id, proxy_type, body_transform, force_pair_creation);
    }

    // Add to shape doubly linked list
    if body_head_shape_id != NULL_INDEX {
        let head_shape = &mut world.shapes[body_head_shape_id as usize];
        head_shape.prev_shape_id = shape_id;
    }

    {
        let shape = &mut world.shapes[shape_id as usize];
        shape.prev_shape_id = NULL_INDEX;
        shape.next_shape_id = body_head_shape_id;
    }
    {
        let body = &mut world.bodies[body_id as usize];
        body.head_shape_id = shape_id;
        body.shape_count += 1;
    }

    if def.is_sensor {
        let sensor_index = world.sensors.len() as i32;
        world.shapes[shape_id as usize].sensor_index = sensor_index;
        world.sensors.push(Sensor {
            hits: Vec::with_capacity(4),
            overlaps1: Vec::with_capacity(16),
            overlaps2: Vec::with_capacity(16),
            shape_id,
        });
    } else {
        world.shapes[shape_id as usize].sensor_index = NULL_INDEX;
    }

    crate::physics_world::validate_solver_sets(world);

    shape_id
}

/// C: static b3CreateShape. The world is explicit; the world0 of body_id is
/// carried into the returned ShapeId like C.
fn create_shape(
    world: &mut World,
    body_id: BodyId,
    def: &ShapeDef,
    geometry: &ShapeGeometryInput,
    transform: Transform,
    scale: Vec3,
    have_transform: bool,
) -> ShapeId {
    b3_assert!(def.internal_value == SECRET_COOKIE);
    b3_assert!(is_valid_float(def.density) && def.density >= 0.0);
    b3_assert!(is_valid_float(def.base_material.friction) && def.base_material.friction >= 0.0);
    b3_assert!(is_valid_float(def.base_material.restitution) && def.base_material.restitution >= 0.0);

    // C: b3GetUnlockedWorld returns NULL when locked.
    if world.locked {
        return NULL_SHAPE_ID;
    }

    if world.shapes.len() as i32 == MAX_SHAPES && world.shape_id_pool.free_array.is_empty() {
        b3_assert!(false);
        return NULL_SHAPE_ID;
    }

    let shape_type = geometry.shape_type();

    let body_idx = crate::body::get_body_full_id(world, body_id);
    if world.bodies[body_idx as usize].body_type != BodyType::Static
        && (shape_type == ShapeType::Compound || shape_type == ShapeType::Height)
    {
        // Compound and height shapes must be on static bodies.
        return NULL_SHAPE_ID;
    }

    world.locked = true;

    let body_transform = crate::body::get_body_transform_quick(world, body_idx);

    let shape_idx = create_shape_internal(world, body_idx, body_transform, def, geometry, transform, scale, have_transform);

    if shape_idx == NULL_INDEX {
        world.locked = false;
        return NULL_SHAPE_ID;
    }

    if def.update_body_mass {
        crate::body::update_body_mass_data(world, body_idx);
    }

    crate::physics_world::validate_solver_sets(world);

    let shape = &world.shapes[shape_idx as usize];
    let id = ShapeId {
        index1: shape.id + 1,
        world0: body_id.world0,
        generation: shape.generation,
    };

    world.locked = false;

    id
}

pub fn create_sphere_shape(world: &mut World, body_id: BodyId, def: &ShapeDef, sphere: &Sphere) -> ShapeId {
    let shape_id = create_shape(
        world,
        body_id,
        def,
        &ShapeGeometryInput::Sphere(sphere),
        Transform::IDENTITY,
        Vec3::ONE,
        false,
    );
    if shape_id.index1 != 0 {
        crate::recording::rec_op(world, crate::recording::RecOp::CreateSphereShape, |b| {
            b.w_bodyid(body_id);
            b.w_shapedef(def);
            b.w_sphere(sphere);
            b.w_shapeid(shape_id);
        });
    }
    shape_id
}

pub fn create_capsule_shape(world: &mut World, body_id: BodyId, def: &ShapeDef, capsule: &Capsule) -> ShapeId {
    let length_sqr = crate::math_functions::distance_squared(capsule.center1, capsule.center2);
    if length_sqr <= crate::constants::linear_slop() * crate::constants::linear_slop() {
        let sphere = Sphere {
            center: lerp(capsule.center1, capsule.center2, 0.5),
            radius: capsule.radius,
        };
        let shape_id = create_shape(
            world,
            body_id,
            def,
            &ShapeGeometryInput::Sphere(&sphere),
            Transform::IDENTITY,
            Vec3::ONE,
            false,
        );
        if shape_id.index1 != 0 {
            // Degenerate capsule becomes a sphere; record what was actually created (C parity).
            crate::recording::rec_op(world, crate::recording::RecOp::CreateSphereShape, |b| {
                b.w_bodyid(body_id);
                b.w_shapedef(def);
                b.w_sphere(&sphere);
                b.w_shapeid(shape_id);
            });
        }
        shape_id
    } else {
        let shape_id = create_shape(
            world,
            body_id,
            def,
            &ShapeGeometryInput::Capsule(capsule),
            Transform::IDENTITY,
            Vec3::ONE,
            false,
        );
        if shape_id.index1 != 0 {
            crate::recording::rec_op(world, crate::recording::RecOp::CreateCapsuleShape, |b| {
                b.w_bodyid(body_id);
                b.w_shapedef(def);
                b.w_capsule(capsule);
                b.w_shapeid(shape_id);
            });
        }
        shape_id
    }
}

pub fn create_hull_shape(world: &mut World, body_id: BodyId, def: &ShapeDef, hull: &Arc<HullData>) -> ShapeId {
    b3_validate!(crate::hull::is_valid_hull(hull));
    b3_validate!(hull.hash != 0);
    let shape_id = create_shape(
        world,
        body_id,
        def,
        &ShapeGeometryInput::Hull(hull),
        Transform::IDENTITY,
        Vec3::ONE,
        false,
    );
    if shape_id.index1 != 0 {
        crate::recording::with_recording(world, |rec| {
            let geometry_id = crate::recording::rec_intern_hull(rec, hull);
            crate::recording::rec_begin_record(rec, crate::recording::RecOp::CreateHullShape as u8);
            rec.buffer.w_bodyid(body_id);
            rec.buffer.w_shapedef(def);
            rec.buffer.w_geomid(geometry_id);
            rec.buffer.w_shapeid(shape_id);
            crate::recording::rec_end_record(rec);
        });
    }
    shape_id
}

pub fn create_transformed_hull_shape(
    world: &mut World,
    body_id: BodyId,
    def: &ShapeDef,
    hull: &Arc<HullData>,
    transform: Transform,
    scale: Vec3,
) -> ShapeId {
    b3_validate!(crate::hull::is_valid_hull(hull));
    let shape_id = create_shape(world, body_id, def, &ShapeGeometryInput::Hull(hull), transform, scale, true);
    if shape_id.index1 != 0 && crate::recording::rec_active(world) {
        // The transform and scale are baked into fresh hull data at create time.
        // Record the baked hull as a plain hull shape so replay rebuilds
        // identical geometry with no rebake (C parity).
        let baked = match &world.shapes[(shape_id.index1 - 1) as usize].geom {
            ShapeGeometry::Hull(h) => Arc::clone(h),
            _ => unreachable!(),
        };
        crate::recording::with_recording(world, |rec| {
            let geometry_id = crate::recording::rec_intern_hull(rec, &baked);
            crate::recording::rec_begin_record(rec, crate::recording::RecOp::CreateHullShape as u8);
            rec.buffer.w_bodyid(body_id);
            rec.buffer.w_shapedef(def);
            rec.buffer.w_geomid(geometry_id);
            rec.buffer.w_shapeid(shape_id);
            crate::recording::rec_end_record(rec);
        });
    }
    shape_id
}

pub fn create_mesh_shape(
    world: &mut World,
    body_id: BodyId,
    def: &ShapeDef,
    mesh_data: &Arc<crate::types::MeshData>,
    scale: Vec3,
) -> ShapeId {
    b3_validate!(crate::mesh::is_valid_mesh(mesh_data));
    b3_validate!(mesh_data.hash != 0);
    let shape_id = create_shape(
        world,
        body_id,
        def,
        &ShapeGeometryInput::Mesh(mesh_data),
        Transform::IDENTITY,
        scale,
        true,
    );
    if shape_id.index1 != 0 {
        crate::recording::with_recording(world, |rec| {
            let geometry_id = crate::recording::rec_intern_mesh(rec, mesh_data);
            crate::recording::rec_begin_record(rec, crate::recording::RecOp::CreateMeshShape as u8);
            rec.buffer.w_bodyid(body_id);
            rec.buffer.w_shapedef(def);
            rec.buffer.w_geomid(geometry_id);
            rec.buffer.w_vec3(scale);
            rec.buffer.w_shapeid(shape_id);
            crate::recording::rec_end_record(rec);
        });
    }
    shape_id
}

pub fn create_height_field_shape(
    world: &mut World,
    body_id: BodyId,
    def: &ShapeDef,
    height_field: &Arc<HeightFieldData>,
) -> ShapeId {
    b3_validate!(height_field.hash != 0);
    let shape_id = create_shape(
        world,
        body_id,
        def,
        &ShapeGeometryInput::HeightField(height_field),
        Transform::IDENTITY,
        Vec3::ONE,
        false,
    );
    if shape_id.index1 != 0 {
        crate::recording::with_recording(world, |rec| {
            let geometry_id = crate::recording::rec_intern_height_field(rec, height_field);
            crate::recording::rec_begin_record(rec, crate::recording::RecOp::CreateHeightFieldShape as u8);
            rec.buffer.w_bodyid(body_id);
            rec.buffer.w_shapedef(def);
            rec.buffer.w_geomid(geometry_id);
            rec.buffer.w_shapeid(shape_id);
            crate::recording::rec_end_record(rec);
        });
    }
    shape_id
}

pub fn create_compound_shape(world: &mut World, body_id: BodyId, def: &ShapeDef, compound: &Arc<CompoundData>) -> ShapeId {
    let shape_id = create_shape(
        world,
        body_id,
        def,
        &ShapeGeometryInput::Compound(compound),
        Transform::IDENTITY,
        Vec3::ONE,
        false,
    );
    if shape_id.index1 != 0 {
        crate::recording::with_recording(world, |rec| {
            let geometry_id = crate::recording::rec_intern_compound(rec, compound);
            crate::recording::rec_begin_record(rec, crate::recording::RecOp::CreateCompoundShape as u8);
            rec.buffer.w_bodyid(body_id);
            rec.buffer.w_shapedef(def);
            rec.buffer.w_geomid(geometry_id);
            rec.buffer.w_shapeid(shape_id);
            crate::recording::rec_end_record(rec);
        });
    }
    shape_id
}

// ---------------------------------------------------------------------------
// Hull database (C: b3AddHullToDatabase / b3RemoveHullFromDatabase in
// physics_world.c, kept here with the only callers).
//
// The C database reference-counts content-deduplicated heap hulls. The port
// keeps the same explicit reference count (one per shape using the entry);
// Arc handles the actual memory lifetime, the count decides when the world
// stops tracking the hull.
// ---------------------------------------------------------------------------

pub fn add_hull_to_database(world: &mut World, hull: &Arc<HullData>) -> Arc<HullData> {
    let key = crate::hull::hash_hull_data(hull) as u32;
    let bucket = world.hull_database.entry(key).or_default();
    for (candidate, ref_count) in bucket.iter_mut() {
        if crate::hull::compare_hull_data(candidate, hull) {
            *ref_count += 1;
            return Arc::clone(candidate);
        }
    }

    bucket.push((Arc::clone(hull), 1));
    Arc::clone(hull)
}

pub fn remove_hull_from_database(world: &mut World, hull: &Arc<HullData>) {
    let key = crate::hull::hash_hull_data(hull) as u32;
    if let Some(bucket) = world.hull_database.get_mut(&key) {
        if let Some(pos) = bucket.iter().position(|(candidate, _)| Arc::ptr_eq(candidate, hull)) {
            // Explicit reference count, one per shape (C semantics). The entry
            // is released when the count reaches zero.
            bucket[pos].1 -= 1;
            if bucket[pos].1 == 0 {
                bucket.remove(pos);
                if bucket.is_empty() {
                    world.hull_database.remove(&key);
                }
            }
        }
    }
}

// C: b3DestroyShapeInternal. Destroy a shape on a body. This doesn't need to be
// called when destroying a body.
pub fn destroy_shape_internal(world: &mut World, shape_id: i32, body_id: i32, wake_bodies: bool) {
    let (prev_shape_id, next_shape_id) = {
        let shape = &world.shapes[shape_id as usize];
        (shape.prev_shape_id, shape.next_shape_id)
    };

    // Remove the shape from the body's doubly linked list.
    if prev_shape_id != NULL_INDEX {
        world.shapes[prev_shape_id as usize].next_shape_id = next_shape_id;
    }

    if next_shape_id != NULL_INDEX {
        world.shapes[next_shape_id as usize].prev_shape_id = prev_shape_id;
    }

    if shape_id == world.bodies[body_id as usize].head_shape_id {
        world.bodies[body_id as usize].head_shape_id = next_shape_id;
    }

    world.bodies[body_id as usize].shape_count -= 1;

    // Remove from broad-phase.
    destroy_shape_proxy(world, shape_id);

    // Destroy any contacts associated with the shape.
    let mut contact_key = world.bodies[body_id as usize].head_contact_key;
    while contact_key != NULL_INDEX {
        let contact_id = contact_key >> 1;
        let edge_index = contact_key & 1;

        let (next_key, shape_id_a, shape_id_b) = {
            let contact = &world.contacts[contact_id as usize];
            (contact.edges[edge_index as usize].next_key, contact.shape_id_a, contact.shape_id_b)
        };
        contact_key = next_key;

        if shape_id_a == shape_id || shape_id_b == shape_id {
            crate::contact::destroy_contact(world, contact_id, wake_bodies);
        }
    }

    let sensor_index = world.shapes[shape_id as usize].sensor_index;
    if sensor_index != NULL_INDEX {
        let generation = world.shapes[shape_id as usize].generation;
        let overlap_count = world.sensors[sensor_index as usize].overlaps2.len();
        for i in 0..overlap_count {
            let visitor: Visitor = world.sensors[sensor_index as usize].overlaps2[i];
            let event = crate::types::SensorEndTouchEvent {
                sensor_shape_id: ShapeId {
                    index1: shape_id + 1,
                    world0: world.world_id,
                    generation,
                },
                visitor_shape_id: ShapeId {
                    index1: visitor.shape_id + 1,
                    world0: world.world_id,
                    generation: visitor.generation,
                },
            };

            world.sensor_end_events[world.end_event_array_index as usize].push(event);
        }

        // Destroy sensor (Vec drops handle the arrays)
        let moved_index = crate::container::array_remove_swap(&mut world.sensors, sensor_index);
        if moved_index != NULL_INDEX {
            // Fixup moved sensor
            let moved_shape_id = world.sensors[sensor_index as usize].shape_id;
            world.shapes[moved_shape_id as usize].sensor_index = sensor_index;
        }
    }

    // Destroy every shape member from b3Alloc
    destroy_shape_allocations(world, shape_id);

    // Return shape to free list.
    free_id(&mut world.shape_id_pool, shape_id);
    world.shapes[shape_id as usize].id = NULL_INDEX;

    crate::physics_world::validate_solver_sets(world);
}

/// C: b3DestroyShape.
pub fn destroy_shape(world: &mut World, shape_id: ShapeId, update_body_mass: bool) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::DestroyShape, |b| {
        b.w_shapeid(shape_id);
        b.w_bool(update_body_mass);
    });

    world.locked = true;

    let shape_idx = get_shape_full_id(world, shape_id);

    // need to wake bodies because this might be a static body
    let wake_bodies = true;

    let body_idx = world.shapes[shape_idx as usize].body_id;
    destroy_shape_internal(world, shape_idx, body_idx, wake_bodies);

    if update_body_mass {
        crate::body::update_body_mass_data(world, body_idx);
    }

    world.locked = false;
}

pub fn compute_shape_aabb(shape: &Shape, transform: Transform) -> AABB {
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::compute_capsule_aabb(capsule, transform),
        ShapeGeometry::Compound(compound) => crate::compound::compute_compound_aabb(compound, transform),
        ShapeGeometry::HeightField(height_field) => {
            crate::height_field::compute_height_field_aabb(height_field, transform)
        }
        ShapeGeometry::Hull(hull) => crate::hull::compute_hull_aabb(hull, transform),
        ShapeGeometry::Mesh(mesh) => crate::mesh::compute_mesh_aabb(&mesh.data, transform, mesh.scale),
        ShapeGeometry::Sphere(sphere) => crate::sphere::compute_sphere_aabb(sphere, transform),
    }
}

#[cfg(not(feature = "double-precision"))]
pub fn compute_fat_shape_aabb(shape: &Shape, transform: WorldTransform, extra: f32) -> AABB {
    let r = vec3(extra, extra, extra);
    // Single precision mode: plain conversion.
    let mut aabb = compute_shape_aabb(shape, transform);
    aabb.lower_bound = sub(aabb.lower_bound, r);
    aabb.upper_bound = add(aabb.upper_bound, r);
    aabb
}

/// Build the box in the body local frame, inflate, then translate by the double origin and
/// round outward. Inflating before the single rounding matters far from the origin where the
/// float margin would otherwise vanish.
#[cfg(feature = "double-precision")]
pub fn compute_fat_shape_aabb(shape: &Shape, transform: WorldTransform, extra: f32) -> AABB {
    let r = vec3(extra, extra, extra);
    let rotation = Transform { p: Vec3::ZERO, q: transform.q };
    let mut local_box = compute_shape_aabb(shape, rotation);
    local_box.lower_bound = sub(local_box.lower_bound, r);
    local_box.upper_bound = add(local_box.upper_bound, r);
    crate::math_functions::offset_aabb(local_box, transform.p)
}

pub fn compute_swept_shape_aabb(shape: &Shape, sweep: &Sweep, time: f32) -> AABB {
    b3_assert!(0.0 <= time && time <= 1.0);
    let xf1 = Transform {
        p: sub(sweep.c1, rotate_vector(sweep.q1, sweep.local_center)),
        q: sweep.q1,
    };
    let xf2 = crate::distance::get_sweep_transform(sweep, time);

    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::compute_swept_capsule_aabb(capsule, xf1, xf2),
        ShapeGeometry::Hull(hull) => crate::hull::compute_swept_hull_aabb(hull, xf1, xf2),
        ShapeGeometry::Sphere(sphere) => crate::sphere::compute_swept_sphere_aabb(sphere, xf1, xf2),
        _ => {
            b3_assert!(false);
            AABB { lower_bound: xf1.p, upper_bound: xf1.p }
        }
    }
}

pub fn get_shape_centroid(shape: &Shape) -> Vec3 {
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => lerp(capsule.center1, capsule.center2, 0.5),
        ShapeGeometry::Compound(compound) => {
            let aabb = crate::compound::compute_compound_aabb(compound, Transform::IDENTITY);
            crate::math_functions::aabb_center(aabb)
        }
        ShapeGeometry::Sphere(sphere) => sphere.center,
        ShapeGeometry::Hull(hull) => hull.center,
        ShapeGeometry::Mesh(mesh) => {
            let aabb = crate::mesh::compute_mesh_aabb(&mesh.data, Transform::IDENTITY, mesh.scale);
            crate::math_functions::aabb_center(aabb)
        }
        ShapeGeometry::HeightField(height_field) => {
            let aabb = crate::height_field::compute_height_field_aabb(height_field, Transform::IDENTITY);
            crate::math_functions::aabb_center(aabb)
        }
    }
}

pub fn get_shape_area(shape: &Shape) -> f32 {
    // todo_erin fix these
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => {
            2.0 * length(sub(capsule.center1, capsule.center2)) + 2.0 * PI * capsule.radius
        }
        ShapeGeometry::Hull(hull) => hull.surface_area,
        ShapeGeometry::Sphere(sphere) => 2.0 * PI * sphere.radius,
        _ => 0.0,
    }
}

// This projects the shape surface area onto a plane
pub fn get_shape_projected_area(shape: &Shape, plane_normal: Vec3) -> f32 {
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => {
            let radius = capsule.radius;
            let axis = sub(capsule.center2, capsule.center1);
            let projected_length = length(cross(axis, plane_normal));
            let cylinder_area = 2.0 * radius * projected_length;
            let sphere_area = PI * radius * radius;
            sphere_area + cylinder_area
        }
        ShapeGeometry::Hull(hull) => crate::hull::compute_hull_projected_area(hull, plane_normal),
        ShapeGeometry::Sphere(sphere) => PI * sphere.radius * sphere.radius,
        _ => 0.0,
    }
}

pub fn compute_shape_mass(shape: &Shape) -> MassData {
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::compute_capsule_mass(capsule, shape.density),
        ShapeGeometry::Hull(hull) => crate::hull::compute_hull_mass(hull, shape.density),
        ShapeGeometry::Sphere(sphere) => crate::sphere::compute_sphere_mass(sphere, shape.density),
        _ => MassData::default(),
    }
}

pub fn compute_shape_extent(shape: &Shape, local_center: Vec3) -> ShapeExtent {
    let mut extent = ShapeExtent::default();

    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => {
            let radius = capsule.radius;
            extent.min_extent = radius;
            let c1 = sub(capsule.center1, local_center);
            let c2 = sub(capsule.center2, local_center);
            let r = vec3(radius, radius, radius);
            extent.max_extent = add(max(c1, c2), r);
        }

        ShapeGeometry::Compound(compound) => {
            // This shouldn't be needed but here for completeness
            let aabb = crate::compound::compute_compound_aabb(compound, Transform::IDENTITY);
            let r1 = length(sub(aabb.lower_bound, local_center));
            let r2 = length(sub(aabb.upper_bound, local_center));
            extent.min_extent = min_float(r1, r2);
            let p = crate::aabb::farthest_point_on_aabb(aabb, local_center);
            extent.max_extent = abs(sub(p, local_center));
        }

        ShapeGeometry::Sphere(sphere) => {
            let radius = sphere.radius;
            extent.min_extent = radius;
            let r = vec3(radius, radius, radius);
            let p = add(sub(sphere.center, local_center), r);
            extent.max_extent = abs(sub(p, local_center));
        }

        ShapeGeometry::Hull(hull) => {
            extent = crate::hull::compute_hull_extent(hull, local_center);
        }

        ShapeGeometry::Mesh(mesh) => {
            // This is needed for kinematic mesh sleeping
            let aabb = crate::mesh::compute_mesh_aabb(&mesh.data, Transform::IDENTITY, mesh.scale);
            let r1 = length(sub(aabb.lower_bound, local_center));
            let r2 = length(sub(aabb.upper_bound, local_center));
            extent.min_extent = min_float(r1, r2);
            let p = crate::aabb::farthest_point_on_aabb(aabb, local_center);
            extent.max_extent = abs(p);
        }

        _ => {}
    }

    extent
}

pub fn ray_cast_shape(shape: &Shape, transform: Transform, input: &RayCastInput) -> CastOutput {
    let local_input = RayCastInput {
        origin: inv_transform_point(transform, input.origin),
        translation: inv_rotate_vector(transform.q, input.translation),
        max_fraction: input.max_fraction,
    };

    let mut output = match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::ray_cast_capsule(capsule, &local_input),
        ShapeGeometry::Compound(compound) => crate::compound::ray_cast_compound(compound, &local_input),
        ShapeGeometry::Sphere(sphere) => crate::sphere::ray_cast_sphere(sphere, &local_input),
        ShapeGeometry::Hull(hull) => crate::hull::ray_cast_hull(hull, &local_input),
        ShapeGeometry::Mesh(mesh) => crate::mesh::ray_cast_mesh(mesh, &local_input),
        ShapeGeometry::HeightField(height_field) => {
            crate::height_field::ray_cast_height_field(height_field, &local_input)
        }
    };

    output.point = transform_point(transform, output.point);
    output.normal = rotate_vector(transform.q, output.normal);
    output
}

pub fn shape_cast_shape(shape: &Shape, transform: Transform, input: &ShapeCastInput) -> CastOutput {
    let mut local_points = [Vec3::ZERO; MAX_SHAPE_CAST_POINTS];

    let count = min_int(input.proxy.count(), MAX_SHAPE_CAST_POINTS as i32);
    for i in 0..count {
        local_points[i as usize] = inv_transform_point(transform, input.proxy.points[i as usize]);
    }

    let local_input = ShapeCastInput {
        proxy: ShapeProxy {
            points: &local_points[..count as usize],
            radius: input.proxy.radius,
        },
        translation: inv_rotate_vector(transform.q, input.translation),
        max_fraction: input.max_fraction,
        can_encroach: input.can_encroach,
    };

    let mut output = match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::shape_cast_capsule(capsule, &local_input),
        ShapeGeometry::Compound(compound) => crate::compound::shape_cast_compound(compound, &local_input),
        ShapeGeometry::HeightField(height_field) => {
            crate::height_field::shape_cast_height_field(height_field, &local_input)
        }
        ShapeGeometry::Hull(hull) => crate::hull::shape_cast_hull(hull, &local_input),
        ShapeGeometry::Mesh(mesh) => crate::mesh::shape_cast_mesh(mesh, &local_input),
        ShapeGeometry::Sphere(sphere) => crate::sphere::shape_cast_sphere(sphere, &local_input),
    };

    output.point = transform_point(transform, output.point);
    output.normal = rotate_vector(transform.q, output.normal);
    output
}

pub fn overlap_shape(shape: &Shape, transform: Transform, proxy: &ShapeProxy) -> bool {
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::overlap_capsule(capsule, transform, proxy),
        ShapeGeometry::Compound(compound) => crate::compound::overlap_compound(compound, transform, proxy),
        ShapeGeometry::HeightField(height_field) => {
            crate::height_field::overlap_height_field(height_field, transform, proxy)
        }
        ShapeGeometry::Hull(hull) => crate::hull::overlap_hull(hull, transform, proxy),
        ShapeGeometry::Mesh(mesh) => crate::mesh::overlap_mesh(mesh, transform, proxy),
        ShapeGeometry::Sphere(sphere) => crate::sphere::overlap_sphere(sphere, transform, proxy),
    }
}

pub fn collide_mover(
    planes: &mut [PlaneResult],
    plane_capacity: i32,
    shape: &Shape,
    transform: Transform,
    mover: &Capsule,
) -> i32 {
    if plane_capacity == 0 {
        return 0;
    }

    let local_mover = Capsule {
        center1: inv_transform_point(transform, mover.center1),
        center2: inv_transform_point(transform, mover.center2),
        radius: mover.radius,
    };

    let plane_count = match &shape.geom {
        ShapeGeometry::Capsule(capsule) => crate::capsule::collide_mover_and_capsule(&mut planes[0], capsule, &local_mover),
        ShapeGeometry::Compound(compound) => {
            crate::compound::collide_mover_and_compound(&mut planes[..plane_capacity as usize], compound, &local_mover)
        }
        ShapeGeometry::Sphere(sphere) => crate::sphere::collide_mover_and_sphere(&mut planes[0], sphere, &local_mover),
        ShapeGeometry::Hull(hull) => crate::hull::collide_mover_and_hull(&mut planes[0], hull, &local_mover),
        ShapeGeometry::Mesh(mesh) => {
            crate::mesh::collide_mover_and_mesh(&mut planes[..plane_capacity as usize], mesh, &local_mover)
        }
        ShapeGeometry::HeightField(height_field) => crate::height_field::collide_mover_and_height_field(
            &mut planes[..plane_capacity as usize],
            height_field,
            &local_mover,
        ),
    };

    for i in 0..plane_count {
        let plane = &mut planes[i as usize];
        plane.plane.normal = rotate_vector(transform.q, plane.plane.normal);
        plane.point = transform_point(transform, plane.point);
    }

    plane_count
}

/// C: b3CreateShapeProxy(shape, bp, type, transform, forcePairCreation).
pub fn create_shape_proxy(
    world: &mut World,
    shape_id: i32,
    body_type: BodyType,
    transform: WorldTransform,
    force_pair_creation: bool,
) {
    b3_assert!(world.shapes[shape_id as usize].proxy_key == NULL_INDEX);

    update_shape_aabbs(&mut world.shapes[shape_id as usize], transform, body_type);

    let (fat_aabb, category_bits) = {
        let shape = &world.shapes[shape_id as usize];
        (shape.fat_aabb, shape.filter.category_bits)
    };

    // Create proxies in the broad-phase.
    let proxy_key = crate::broad_phase::broad_phase_create_proxy(
        &mut world.broad_phase,
        body_type,
        fat_aabb,
        category_bits,
        shape_id,
        force_pair_creation,
    );
    b3_assert!((crate::broad_phase::proxy_type(proxy_key) as usize) < crate::types::BODY_TYPE_COUNT);

    world.shapes[shape_id as usize].proxy_key = proxy_key;
}

/// C: b3DestroyShapeProxy(shape, bp).
pub fn destroy_shape_proxy(world: &mut World, shape_id: i32) {
    let proxy_key = world.shapes[shape_id as usize].proxy_key;
    if proxy_key != NULL_INDEX {
        crate::broad_phase::broad_phase_destroy_proxy(&mut world.broad_phase, proxy_key);
        world.shapes[shape_id as usize].proxy_key = NULL_INDEX;
    }
}

fn destroy_shape_allocation_for_shape_change(world: &mut World, shape_id: i32) {
    let hull = match &world.shapes[shape_id as usize].geom {
        ShapeGeometry::Hull(hull) => Some(Arc::clone(hull)),
        _ => None,
    };

    if let Some(hull) = hull {
        // Drop the shape's reference before pruning the database entry.
        world.shapes[shape_id as usize].geom = ShapeGeometry::default();
        remove_hull_from_database(world, &hull);
    }

    // userShape / debug draw callbacks: not ported.
}

pub fn destroy_shape_allocations(world: &mut World, shape_id: i32) {
    destroy_shape_allocation_for_shape_change(world, shape_id);

    let shape = &mut world.shapes[shape_id as usize];
    shape.materials = Vec::new();

    // Name is stored inline. Sensor data is destroyed elsewhere
}

/// C: b3MakeShapeProxy. The buffer holds the capsule end points (C borrows the
/// adjacent center1/center2 fields directly).
pub fn make_shape_proxy<'a>(shape: &'a Shape, buffer: &'a mut [Vec3; 2]) -> ShapeProxy<'a> {
    match &shape.geom {
        ShapeGeometry::Capsule(capsule) => {
            buffer[0] = capsule.center1;
            buffer[1] = capsule.center2;
            ShapeProxy { points: &buffer[..2], radius: capsule.radius }
        }

        ShapeGeometry::Sphere(sphere) => ShapeProxy {
            points: std::slice::from_ref(&sphere.center),
            radius: sphere.radius,
        },

        ShapeGeometry::Hull(hull) => ShapeProxy { points: &hull.points, radius: 0.0 },

        _ => {
            b3_assert!(false);
            ShapeProxy { points: &[], radius: 0.0 }
        }
    }
}

pub fn make_local_proxy<'a>(proxy: &ShapeProxy, transform: Transform, buffer: &'a mut [Vec3]) -> ShapeProxy<'a> {
    let inv_transform = invert_transform(transform);
    let r: Matrix3 = make_matrix_from_quat(inv_transform.q);

    let count = min_int(proxy.count(), MAX_SHAPE_CAST_POINTS as i32);
    for i in 0..count {
        buffer[i as usize] = add(mul_mv(r, proxy.points[i as usize]), inv_transform.p);
    }

    ShapeProxy {
        points: &buffer[..count as usize],
        radius: proxy.radius,
    }
}

pub fn compute_proxy_aabb(proxy: &ShapeProxy) -> AABB {
    let points = proxy.points;
    let mut aabb = AABB {
        lower_bound: points[0],
        upper_bound: points[0],
    };

    for i in 1..proxy.count() {
        aabb.lower_bound = crate::math_functions::min(aabb.lower_bound, points[i as usize]);
        aabb.upper_bound = max(aabb.upper_bound, points[i as usize]);
    }

    let r = vec3(proxy.radius, proxy.radius, proxy.radius);
    aabb.lower_bound = sub(aabb.lower_bound, r);
    aabb.upper_bound = add(aabb.upper_bound, r);
    aabb
}

// Resolve the user material id for a hit point on the given shape. Mesh/heightfield shapes
// use the manifold-point triangleIndex to pick a per-triangle material. Compound shapes use
// the contact's childIndex to find the participating child, then for a mesh child apply the
// child's materialIndices indirection on top of the per-triangle index. Convex shapes fall
// back to materials[0]. childIndex is unused for non-compound shapes.
pub fn get_shape_user_material_id(shape: &Shape, child_index: i32, triangle_index: i32) -> u64 {
    let material_count = shape_material_count(shape);
    if material_count == 0 {
        return 0;
    }

    let mut material_index: i32 = 0;
    match &shape.geom {
        ShapeGeometry::Mesh(mesh) => {
            let indices = &mesh.data.materials;
            if !indices.is_empty() {
                material_index = indices[triangle_index as usize] as i32;
            }
        }
        ShapeGeometry::HeightField(height_field) => {
            material_index = crate::height_field::get_height_field_material(height_field, triangle_index);
        }
        ShapeGeometry::Compound(compound) => {
            let child = crate::compound::get_compound_child(compound, child_index);
            match &child.geom {
                crate::types::ChildShapeGeom::Mesh(child_mesh) => {
                    let indices = &child_mesh.data.materials;
                    let mut mesh_material_index =
                        if !indices.is_empty() { indices[triangle_index as usize] as i32 } else { 0 };
                    mesh_material_index = clamp_int(mesh_material_index, 0, MAX_COMPOUND_MESH_MATERIALS as i32 - 1);
                    material_index = child.material_indices[mesh_material_index as usize];
                }
                _ => {
                    material_index = child.material_indices[0];
                }
            }
        }
        _ => {}
    }

    material_index = clamp_int(material_index, 0, material_count - 1);
    get_shape_materials(shape)[material_index as usize].user_material_id
}

/// C: static b3ResetProxy.
fn reset_proxy(world: &mut World, shape_id: i32, wake_bodies: bool, destroy_proxy: bool) {
    let body_id = world.shapes[shape_id as usize].body_id;

    // destroy all contacts associated with this shape
    let mut contact_key = world.bodies[body_id as usize].head_contact_key;
    while contact_key != NULL_INDEX {
        let contact_id = contact_key >> 1;
        let edge_index = contact_key & 1;

        let (next_key, shape_id_a, shape_id_b) = {
            let contact = &world.contacts[contact_id as usize];
            (contact.edges[edge_index as usize].next_key, contact.shape_id_a, contact.shape_id_b)
        };
        contact_key = next_key;

        if shape_id_a == shape_id || shape_id_b == shape_id {
            crate::contact::destroy_contact(world, contact_id, wake_bodies);
        }
    }

    let transform = crate::body::get_body_transform_quick(world, body_id);
    let proxy_key = world.shapes[shape_id as usize].proxy_key;
    if proxy_key != NULL_INDEX {
        let proxy_type = crate::broad_phase::proxy_type(proxy_key);
        update_shape_aabbs(&mut world.shapes[shape_id as usize], transform, proxy_type);

        if destroy_proxy {
            crate::broad_phase::broad_phase_destroy_proxy(&mut world.broad_phase, proxy_key);

            let force_pair_creation = true;
            let (fat_aabb, category_bits) = {
                let shape = &world.shapes[shape_id as usize];
                (shape.fat_aabb, shape.filter.category_bits)
            };
            world.shapes[shape_id as usize].proxy_key = crate::broad_phase::broad_phase_create_proxy(
                &mut world.broad_phase,
                proxy_type,
                fat_aabb,
                category_bits,
                shape_id,
                force_pair_creation,
            );
        } else {
            let fat_aabb = world.shapes[shape_id as usize].fat_aabb;
            crate::broad_phase::broad_phase_move_proxy(&mut world.broad_phase, proxy_key, fat_aabb);
        }
    } else {
        let proxy_type = world.bodies[body_id as usize].body_type;
        update_shape_aabbs(&mut world.shapes[shape_id as usize], transform, proxy_type);
    }

    crate::physics_world::validate_solver_sets(world);
}

// ---------------------------------------------------------------------------
// Public b3Shape_* API
// ---------------------------------------------------------------------------

pub fn shape_get_body(world: &World, shape_id: ShapeId) -> BodyId {
    let shape_idx = get_shape_full_id(world, shape_id);
    crate::body::make_body_id(world, world.shapes[shape_idx as usize].body_id)
}

pub fn shape_get_world(world: &World, shape_id: ShapeId) -> WorldId {
    WorldId { index1: shape_id.world0 + 1, generation: world.generation }
}

pub fn shape_set_user_data(world: &mut World, shape_id: ShapeId, user_data: u64) {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].user_data = user_data;
}

pub fn shape_get_user_data(world: &World, shape_id: ShapeId) -> u64 {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].user_data
}

pub fn shape_is_sensor(world: &World, shape_id: ShapeId) -> bool {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].sensor_index != NULL_INDEX
}

// todo no tests
pub fn shape_ray_cast(world: &World, shape_id: ShapeId, origin: crate::math_functions::Pos, translation: Vec3) -> WorldCastOutput {
    b3_assert!(is_valid_position(origin));
    b3_assert!(is_valid_vec3(translation));

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];

    // Re-center on the origin so the cast runs in float precision far from the world origin
    let transform = to_relative_transform(crate::body::get_body_transform(world, shape.body_id), origin);

    // The ray starts at the origin, so its origin in the re-centered frame is zero
    let input = RayCastInput { origin: Vec3::ZERO, translation, max_fraction: 1.0 };

    // Lift the re-centered float result back to a world position
    let local = ray_cast_shape(shape, transform, &input);
    WorldCastOutput {
        normal: local.normal,
        point: offset_pos(origin, local.point),
        fraction: local.fraction,
        iterations: local.iterations,
        triangle_index: local.triangle_index,
        child_index: local.child_index,
        material_index: local.material_index,
        hit: local.hit,
    }
}

pub fn shape_set_density(world: &mut World, shape_id: ShapeId, density: f32, update_body_mass: bool) {
    b3_assert!(is_valid_float(density) && density >= 0.0);

    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetDensity, |b| {
        b.w_shapeid(shape_id);
        b.w_f32(density);
        b.w_bool(update_body_mass);
    });

    let shape_idx = get_shape_full_id(world, shape_id);
    if density == world.shapes[shape_idx as usize].density {
        // early return to avoid expensive function
        return;
    }

    world.shapes[shape_idx as usize].density = density;

    if update_body_mass {
        let body_idx = world.shapes[shape_idx as usize].body_id;
        crate::body::update_body_mass_data(world, body_idx);
    }
}

pub fn shape_get_density(world: &World, shape_id: ShapeId) -> f32 {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].density
}

pub fn shape_set_friction(world: &mut World, shape_id: ShapeId, friction: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetFriction, |b| {
        b.w_shapeid(shape_id);
        b.w_f32(friction);
    });
    b3_assert!(is_valid_float(friction) && friction >= 0.0);
    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &mut world.shapes[shape_idx as usize];
    b3_assert!(shape.shape_type() != ShapeType::Compound);
    get_shape_materials_mut(shape)[0].friction = friction;
}

pub fn shape_get_friction(world: &World, shape_id: ShapeId) -> f32 {
    let shape_idx = get_shape_full_id(world, shape_id);
    get_shape_materials(&world.shapes[shape_idx as usize])[0].friction
}

pub fn shape_set_restitution(world: &mut World, shape_id: ShapeId, restitution: f32) {

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetRestitution, |b| {
        b.w_shapeid(shape_id);
        b.w_f32(restitution);
    });
    b3_assert!(is_valid_float(restitution) && restitution >= 0.0);
    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &mut world.shapes[shape_idx as usize];
    b3_assert!(shape.shape_type() != ShapeType::Compound);
    get_shape_materials_mut(shape)[0].restitution = restitution;
}

pub fn shape_get_restitution(world: &World, shape_id: ShapeId) -> f32 {
    let shape_idx = get_shape_full_id(world, shape_id);
    get_shape_materials(&world.shapes[shape_idx as usize])[0].restitution
}

pub fn shape_set_surface_material(world: &mut World, shape_id: ShapeId, surface_material: SurfaceMaterial) {

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetSurfaceMaterial, |b| {
        b.w_shapeid(shape_id);
        b.w_material(&surface_material);
    });
    b3_assert!(is_valid_float(surface_material.friction) && surface_material.friction >= 0.0);
    b3_assert!(is_valid_float(surface_material.restitution) && surface_material.restitution >= 0.0);
    b3_assert!(is_valid_float(surface_material.rolling_resistance) && surface_material.rolling_resistance >= 0.0);
    b3_assert!(is_valid_vec3(surface_material.tangent_velocity));

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &mut world.shapes[shape_idx as usize];
    b3_assert!(shape.shape_type() != ShapeType::Compound);
    get_shape_materials_mut(shape)[0] = surface_material;
}

pub fn shape_get_surface_material(world: &World, shape_id: ShapeId) -> SurfaceMaterial {
    let shape_idx = get_shape_full_id(world, shape_id);
    get_shape_materials(&world.shapes[shape_idx as usize])[0]
}

pub fn shape_get_mesh_material_count(world: &World, shape_id: ShapeId) -> i32 {
    let shape_idx = get_shape_full_id(world, shape_id);
    shape_material_count(&world.shapes[shape_idx as usize])
}

pub fn shape_set_mesh_material(world: &mut World, shape_id: ShapeId, surface_material: SurfaceMaterial, index: i32) {
    b3_assert!(is_valid_float(surface_material.friction) && surface_material.friction >= 0.0);
    b3_assert!(is_valid_float(surface_material.restitution) && surface_material.restitution >= 0.0);
    b3_assert!(is_valid_float(surface_material.rolling_resistance) && surface_material.rolling_resistance >= 0.0);
    b3_assert!(is_valid_vec3(surface_material.tangent_velocity));

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &mut world.shapes[shape_idx as usize];

    b3_assert!(0 <= index && index < shape_material_count(shape));
    b3_assert!(shape.shape_type() != ShapeType::Compound);
    get_shape_materials_mut(shape)[index as usize] = surface_material;
}

pub fn shape_get_mesh_surface_material(world: &World, shape_id: ShapeId, index: i32) -> SurfaceMaterial {
    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];
    b3_assert!(0 <= index && index < shape_material_count(shape));
    get_shape_materials(shape)[index as usize]
}

pub fn shape_get_filter(world: &World, shape_id: ShapeId) -> Filter {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].filter
}

pub fn shape_set_filter(world: &mut World, shape_id: ShapeId, filter: Filter, invoke_contacts: bool) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetFilter, |b| {
        b.w_shapeid(shape_id);
        b.w_filter(filter);
        b.w_bool(invoke_contacts);
    });

    let shape_idx = get_shape_full_id(world, shape_id);
    {
        let shape = &world.shapes[shape_idx as usize];
        if filter.mask_bits == shape.filter.mask_bits
            && filter.category_bits == shape.filter.category_bits
            && filter.group_index == shape.filter.group_index
        {
            return;
        }
    }

    world.shapes[shape_idx as usize].filter = filter;

    if invoke_contacts {
        world.locked = true;
        let wake_bodies = true;

        // If the category bits change, I need to destroy the proxy because it affects
        // the tree sorting. (Note: preserved from C, where this compares the filter
        // against itself after the assignment above and is therefore always true.)
        let destroy_proxy = filter.category_bits == world.shapes[shape_idx as usize].filter.category_bits;

        // need to wake bodies because a filter change may destroy contacts
        reset_proxy(world, shape_idx, wake_bodies, destroy_proxy);
        world.locked = false;
    }

    // note: this does not immediately update sensor overlaps. Instead sensor
    // overlaps are updated the next time step
}

pub fn shape_enable_sensor_events(world: &mut World, shape_id: ShapeId, flag: bool) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeEnableSensorEvents, |b| {
        b.w_shapeid(shape_id);
        b.w_bool(flag);
    });
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_sensor_events = flag;
}

pub fn shape_are_sensor_events_enabled(world: &World, shape_id: ShapeId) -> bool {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_sensor_events
}

pub fn shape_enable_contact_events(world: &mut World, shape_id: ShapeId, flag: bool) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeEnableContactEvents, |b| {
        b.w_shapeid(shape_id);
        b.w_bool(flag);
    });
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_contact_events = flag;
}

pub fn shape_are_contact_events_enabled(world: &World, shape_id: ShapeId) -> bool {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_contact_events
}

pub fn shape_enable_pre_solve_events(world: &mut World, shape_id: ShapeId, flag: bool) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeEnablePreSolveEvents, |b| {
        b.w_shapeid(shape_id);
        b.w_bool(flag);
    });
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_pre_solve_events = flag;
}

pub fn shape_are_pre_solve_events_enabled(world: &World, shape_id: ShapeId) -> bool {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_pre_solve_events
}

pub fn shape_enable_hit_events(world: &mut World, shape_id: ShapeId, flag: bool) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeEnableHitEvents, |b| {
        b.w_shapeid(shape_id);
        b.w_bool(flag);
    });
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_hit_events = flag;
}

pub fn shape_are_hit_events_enabled(world: &World, shape_id: ShapeId) -> bool {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].enable_hit_events
}

pub fn shape_get_type(world: &World, shape_id: ShapeId) -> ShapeType {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].shape_type()
}

pub fn shape_get_sphere(world: &World, shape_id: ShapeId) -> Sphere {
    let shape_idx = get_shape_full_id(world, shape_id);
    *world.shapes[shape_idx as usize].as_sphere()
}

pub fn shape_get_capsule(world: &World, shape_id: ShapeId) -> Capsule {
    let shape_idx = get_shape_full_id(world, shape_id);
    *world.shapes[shape_idx as usize].as_capsule()
}

pub fn shape_get_hull(world: &World, shape_id: ShapeId) -> Arc<HullData> {
    let shape_idx = get_shape_full_id(world, shape_id);
    Arc::clone(world.shapes[shape_idx as usize].as_hull())
}

pub fn shape_get_mesh(world: &World, shape_id: ShapeId) -> Mesh {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].as_mesh().clone()
}

pub fn shape_get_height_field(world: &World, shape_id: ShapeId) -> Arc<HeightFieldData> {
    let shape_idx = get_shape_full_id(world, shape_id);
    Arc::clone(world.shapes[shape_idx as usize].as_height_field())
}

pub fn shape_set_sphere(world: &mut World, shape_id: ShapeId, sphere: &Sphere) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetSphere, |b| {
        b.w_shapeid(shape_id);
        b.w_sphere(sphere);
    });

    world.locked = true;

    let shape_idx = get_shape_full_id(world, shape_id);

    destroy_shape_allocation_for_shape_change(world, shape_idx);

    {
        let shape = &mut world.shapes[shape_idx as usize];
        shape.geom = ShapeGeometry::Sphere(*sphere);
        shape.aabb_margin = compute_shape_margin(shape);
    }

    // need to wake bodies so they can react to the shape change
    let wake_bodies = true;
    let destroy_proxy = true;
    reset_proxy(world, shape_idx, wake_bodies, destroy_proxy);

    world.locked = false;
}

pub fn shape_set_capsule(world: &mut World, shape_id: ShapeId, capsule: &Capsule) {
    if world.locked {
        return;
    }

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeSetCapsule, |b| {
        b.w_shapeid(shape_id);
        b.w_capsule(capsule);
    });

    world.locked = true;

    let shape_idx = get_shape_full_id(world, shape_id);

    destroy_shape_allocation_for_shape_change(world, shape_idx);

    {
        let shape = &mut world.shapes[shape_idx as usize];
        shape.geom = ShapeGeometry::Capsule(*capsule);
        shape.aabb_margin = compute_shape_margin(shape);
    }

    // need to wake bodies so they can react to the shape change
    let wake_bodies = true;
    let destroy_proxy = true;
    reset_proxy(world, shape_idx, wake_bodies, destroy_proxy);

    world.locked = false;
}

pub fn shape_set_hull(world: &mut World, shape_id: ShapeId, hull: &Arc<HullData>) {
    b3_validate!(crate::hull::is_valid_hull(hull));
    b3_validate!(hull.hash != 0);

    if world.locked {
        return;
    }

    world.locked = true;

    let shape_idx = get_shape_full_id(world, shape_id);

    // Acquire the new hull before releasing the old so the input may safely alias
    // the shape's current shared data.
    let data = add_hull_to_database(world, hull);

    // Same shared hull, avoid destroying contacts and recreating the proxy
    {
        let shape = &world.shapes[shape_idx as usize];
        if let ShapeGeometry::Hull(current) = &shape.geom {
            if Arc::ptr_eq(current, &data) {
                remove_hull_from_database(world, &data);
                world.locked = false;
                return;
            }
        }
    }

    destroy_shape_allocation_for_shape_change(world, shape_idx);

    {
        let shape = &mut world.shapes[shape_idx as usize];
        shape.geom = ShapeGeometry::Hull(data);
        shape.aabb_margin = compute_shape_margin(shape);
    }

    // need to wake bodies so they can react to the shape change
    let wake_bodies = true;
    let destroy_proxy = true;
    reset_proxy(world, shape_idx, wake_bodies, destroy_proxy);

    world.locked = false;
}

pub fn shape_set_mesh(world: &mut World, shape_id: ShapeId, mesh_data: &Arc<crate::types::MeshData>, scale: Vec3) {
    b3_assert!(is_valid_vec3(scale));
    b3_assert!(crate::mesh::is_valid_mesh(mesh_data));

    if world.locked {
        return;
    }

    world.locked = true;

    let shape_idx = get_shape_full_id(world, shape_id);

    destroy_shape_allocation_for_shape_change(world, shape_idx);

    {
        let shape = &mut world.shapes[shape_idx as usize];
        shape.geom = ShapeGeometry::Mesh(Mesh {
            data: Arc::clone(mesh_data),
            scale: safe_scale(scale),
        });
        shape.aabb_margin = compute_shape_margin(shape);
    }

    // need to wake bodies so they can react to the shape change
    let wake_bodies = true;
    let destroy_proxy = true;
    reset_proxy(world, shape_idx, wake_bodies, destroy_proxy);

    world.locked = false;
}

pub fn shape_get_contact_capacity(world: &World, shape_id: ShapeId) -> i32 {
    if world.locked {
        return 0;
    }

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];
    if shape.sensor_index != NULL_INDEX {
        return 0;
    }

    // Conservative and fast
    world.bodies[shape.body_id as usize].contact_count
}

/// C: b3Shape_GetContactData(shapeId, contactData, capacity) fills a caller
/// array; the port returns a Vec (ContactData borrows the world's manifolds).
pub fn shape_get_contact_data<'a>(world: &'a World, shape_id: ShapeId, capacity: i32) -> Vec<ContactData<'a>> {
    let mut result = Vec::new();

    if world.locked {
        return result;
    }

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];
    if shape.sensor_index != NULL_INDEX {
        return result;
    }

    let body = &world.bodies[shape.body_id as usize];
    let mut contact_key = body.head_contact_key;
    while contact_key != NULL_INDEX && (result.len() as i32) < capacity {
        let contact_id = contact_key >> 1;
        let edge_index = contact_key & 1;

        let contact = &world.contacts[contact_id as usize];

        // Does contact involve this shape and is it touching?
        if (contact.shape_id_a == shape_id.index1 - 1 || contact.shape_id_b == shape_id.index1 - 1)
            && (contact.flags & crate::contact::CONTACT_TOUCHING_FLAG) != 0
        {
            let shape_a = &world.shapes[contact.shape_id_a as usize];
            let shape_b = &world.shapes[contact.shape_id_b as usize];

            result.push(ContactData {
                contact_id: ContactId {
                    index1: contact.contact_id + 1,
                    world0: shape_id.world0,
                    generation: contact.generation,
                },
                shape_id_a: ShapeId {
                    index1: shape_a.id + 1,
                    world0: shape_id.world0,
                    generation: shape_a.generation,
                },
                shape_id_b: ShapeId {
                    index1: shape_b.id + 1,
                    world0: shape_id.world0,
                    generation: shape_b.generation,
                },
                manifolds: &contact.manifolds,
            });
        }

        contact_key = contact.edges[edge_index as usize].next_key;
    }

    b3_assert!(result.len() as i32 <= capacity);

    result
}

pub fn shape_get_sensor_capacity(world: &World, shape_id: ShapeId) -> i32 {
    if world.locked {
        return 0;
    }

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];
    if shape.sensor_index == NULL_INDEX {
        return 0;
    }

    world.sensors[shape.sensor_index as usize].overlaps2.len() as i32
}

pub fn shape_get_sensor_data(world: &World, shape_id: ShapeId, visitor_ids: &mut [ShapeId]) -> i32 {
    if world.locked {
        return 0;
    }

    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];
    if shape.sensor_index == NULL_INDEX {
        return 0;
    }

    let sensor = &world.sensors[shape.sensor_index as usize];

    let count = min_int(sensor.overlaps2.len() as i32, visitor_ids.len() as i32);
    for i in 0..count {
        let visitor = sensor.overlaps2[i as usize];
        visitor_ids[i as usize] = ShapeId {
            index1: visitor.shape_id + 1,
            world0: shape_id.world0,
            generation: visitor.generation,
        };
    }

    count
}

pub fn shape_get_aabb(world: &World, shape_id: ShapeId) -> AABB {
    let shape_idx = get_shape_full_id(world, shape_id);
    world.shapes[shape_idx as usize].aabb
}

pub fn shape_compute_mass_data(world: &World, shape_id: ShapeId) -> MassData {
    let shape_idx = get_shape_full_id(world, shape_id);
    compute_shape_mass(&world.shapes[shape_idx as usize])
}

pub fn shape_get_closest_point(world: &World, shape_id: ShapeId, target: Vec3) -> Vec3 {
    let shape_idx = get_shape_full_id(world, shape_id);
    let shape = &world.shapes[shape_idx as usize];
    // Low level closest point query is a documented float carve-out far from the origin
    let transform = to_relative_transform(crate::body::get_body_transform_quick(world, shape.body_id), POS_ZERO);

    let mut buffer = [Vec3::ZERO; 2];
    let input = DistanceInput {
        proxy_a: make_shape_proxy(shape, &mut buffer),
        proxy_b: ShapeProxy { points: std::slice::from_ref(&target), radius: 0.0 },
        transform: inv_mul_transforms(transform, Transform::IDENTITY),
        use_radii: true,
    };

    let mut cache = SimplexCache::default();
    let output = crate::distance::shape_distance(&input, &mut cache, None);

    // Witness point comes back in frame A, lift it back to the query frame
    transform_point(transform, output.point_a)
}

// https://en.wikipedia.org/wiki/Density_of_air
// https://www.engineeringtoolbox.com/wind-load-d_1775.html
// force = 0.5 * air_density * velocity^2 * area
// https://en.wikipedia.org/wiki/Lift_(force)
pub fn shape_apply_wind(world: &mut World, shape_id: ShapeId, wind: Vec3, drag: f32, lift: f32, max_speed: f32, wake: bool) {

    crate::recording::rec_op(world, crate::recording::RecOp::ShapeApplyWind, |b| {
        b.w_shapeid(shape_id);
        b.w_vec3(wind);
        b.w_f32(drag);
        b.w_f32(lift);
        b.w_f32(max_speed);
        b.w_bool(wake);
    });
    let shape_idx = get_shape_full_id(world, shape_id);

    let shape_type = world.shapes[shape_idx as usize].shape_type();
    if shape_type != ShapeType::Sphere && shape_type != ShapeType::Capsule && shape_type != ShapeType::Hull {
        return;
    }

    let body_id = world.shapes[shape_idx as usize].body_id;

    {
        let body = &world.bodies[body_id as usize];

        if body.body_type != BodyType::Dynamic {
            return;
        }

        if body.set_index == DISABLED_SET {
            return;
        }

        if body.set_index >= FIRST_SLEEPING_SET && !wake {
            return;
        }
    }

    if world.bodies[body_id as usize].set_index != AWAKE_SET {
        // Must wake for state to exist
        crate::body::wake_body_with_lock(world, body_id);
    }

    b3_assert!(world.bodies[body_id as usize].set_index == AWAKE_SET);

    // C caches the sim pointer before waking; the port reads it after (see header).
    let sim = *crate::body::get_body_sim_from_id(world, body_id);
    let state = *crate::body::get_body_state_from_id_mut(world, body_id).expect("awake body must have state");

    // Only the rotation is used below, so the demoted world transform is exact
    let transform = to_relative_transform(sim.transform, POS_ZERO);

    let length_units = get_length_units_per_meter();
    let volume_units = length_units * length_units * length_units;

    let air_density = 1.2250 / volume_units;

    let mut force = Vec3::ZERO;
    let mut torque = Vec3::ZERO;

    let geom = world.shapes[shape_idx as usize].geom.clone();
    let local_centroid = world.shapes[shape_idx as usize].local_centroid;

    match &geom {
        ShapeGeometry::Sphere(sphere) => {
            let radius = sphere.radius;
            let centroid = local_centroid;
            let lever = rotate_vector(transform.q, sub(centroid, sim.local_center));
            let shape_velocity = add(state.linear_velocity, cross(state.angular_velocity, lever));
            let relative_velocity = mul_sub(wind, drag, shape_velocity);
            let mut speed = 0.0;
            let direction = get_length_and_normalize(&mut speed, relative_velocity);
            speed = min_float(speed, max_speed);
            let projected_area = PI * radius * radius;
            force = mul_sv(0.5 * air_density * projected_area * speed * speed, direction);
            torque = cross(lever, force);
        }

        ShapeGeometry::Capsule(capsule) => {
            let centroid = local_centroid;
            let lever = rotate_vector(transform.q, sub(centroid, sim.local_center));
            let shape_velocity = add(state.linear_velocity, cross(state.angular_velocity, lever));
            let relative_velocity = mul_sub(wind, drag, shape_velocity);
            let mut speed = 0.0;
            let direction = get_length_and_normalize(&mut speed, relative_velocity);
            speed = min_float(speed, max_speed);

            let mut d = sub(capsule.center2, capsule.center1);
            d = rotate_vector(transform.q, d);

            let radius = capsule.radius;
            let projected_area = PI * radius * radius + 2.0 * radius * length(cross(d, direction));

            // Normal that opposes the wind
            let e = normalize(d);
            let normal = sub(mul_sv(dot(direction, e), e), direction);

            // portion of wind that is perpendicular to surface
            let lift_direction = cross(cross(normal, direction), direction);

            let force_magnitude = 0.5 * air_density * projected_area * speed * speed;
            force = mul_sv(force_magnitude, mul_add(direction, lift, lift_direction));

            let edge_lever = mul_add(lever, radius, normal);
            torque = cross(edge_lever, force);
        }

        ShapeGeometry::Hull(hull) => {
            let matrix = make_matrix_from_quat(transform.q);

            let face_count = hull.face_count();
            let points = &hull.points;
            let faces = &hull.faces;
            let edges = &hull.edges;
            let planes = &hull.planes;

            let linear_velocity = state.linear_velocity;
            let angular_velocity = state.angular_velocity;
            let local_center_of_mass = sim.local_center;

            for i in 0..face_count {
                let face = faces[i as usize];
                let edge1_index = face.edge as usize;
                let edge1 = edges[edge1_index];
                let mut edge2_index = edge1.next as usize;
                let edge2 = edges[edge2_index];
                let mut edge3_index = edges[edge2_index].next as usize;

                b3_assert!(edge1_index != edge3_index);
                b3_assert!((edge1.origin as i32) < hull.vertex_count());
                b3_assert!((edge2.origin as i32) < hull.vertex_count());

                let local_point1 = points[edge1.origin as usize];
                let mut local_point2 = points[edge2.origin as usize];
                let v1 = mul_mv(matrix, local_point1);
                let mut v2 = mul_mv(matrix, local_point2);
                let normal = mul_mv(matrix, planes[i as usize].normal);

                loop {
                    let edge3 = edges[edge3_index];
                    b3_assert!((edge3.origin as i32) < hull.vertex_count());
                    let local_point3 = points[edge3.origin as usize];
                    let v3 = mul_mv(matrix, local_point3);

                    // Triangle center
                    let local_center = mul_sv(0.333333, add(local_point1, add(local_point2, local_point3)));

                    // Lever arm from center of mass to triangle center in world space
                    let lever = mul_mv(matrix, sub(local_center, local_center_of_mass));

                    // Velocity of the triangle center in world space
                    let center_velocity = add(linear_velocity, cross(angular_velocity, lever));

                    let relative_velocity = mul_sub(wind, drag, center_velocity);
                    let mut speed = 0.0;
                    let direction = get_length_and_normalize(&mut speed, relative_velocity);

                    // Check for back-side
                    if dot(normal, direction) < -f32::EPSILON {
                        let projected_area = -0.5 * dot(cross(sub(v2, v1), sub(v3, v1)), direction);
                        b3_validate!(projected_area >= -f32::EPSILON);

                        let lift_direction = cross(cross(normal, direction), direction);

                        speed = min_float(speed, max_speed);

                        let force_magnitude = 0.5 * air_density * projected_area * speed * speed;
                        let delta_force = mul_sv(force_magnitude, mul_add(direction, lift, lift_direction));
                        let delta_torque = cross(lever, delta_force);

                        force = add(force, delta_force);
                        torque = add(torque, delta_torque);
                    }

                    edge2_index = edge3_index;
                    let _ = edge2_index;
                    edge3_index = edges[edge3_index].next as usize;
                    v2 = v3;
                    local_point2 = local_point3;

                    if edge1_index == edge3_index {
                        break;
                    }
                }
            }
        }

        _ => {}
    }

    let sim_mut = crate::body::get_body_sim_from_id_mut(world, body_id);
    sim_mut.force = add(sim_mut.force, force);
    sim_mut.torque = add(sim_mut.torque, torque);
}

// ---------------------------------------------------------------------------
// Shape time of impact (CCD)
// ---------------------------------------------------------------------------

/// The C b3MeshImpactContext. The C toiInput lives in the context with proxyA
/// mutated per triangle; the port rebuilds the input per call from these parts.
struct MeshImpactContext<'a> {
    proxy_b: ShapeProxy<'a>,
    sweep_a: Sweep,
    sweep_b: Sweep,
    max_fraction: f32,
    toi_output: TOIOutput,
    // Centroid of shape in body B local space
    local_centroid_b: Vec3,
    // Centroid of shape at beginning and end of sweep in mesh local space. Used for early out.
    mesh_local_centroid_b1: Vec3,
    mesh_local_centroid_b2: Vec3,
    fallback_radius: f32,
    is_sensor: bool,

    visit_count: i32,
}

// C: b3MeshTimeOfImpactFcn (b3MeshQueryFcn)
fn mesh_time_of_impact_fcn(ctx: &mut MeshImpactContext, a: Vec3, b: Vec3, c: Vec3, _triangle_index: i32) -> bool {
    ctx.visit_count += 1;

    // Early out for parallel movement
    let c1 = ctx.mesh_local_centroid_b1;
    let c2 = ctx.mesh_local_centroid_b2;

    let n = normalize(cross(sub(b, a), sub(c, a)));
    let offset1 = dot(n, sub(c1, a));
    let offset2 = dot(n, sub(c2, a));

    if offset1 < 0.0 {
        // Started behind or finished in front
        return true;
    }

    if !ctx.is_sensor && offset1 - offset2 < ctx.fallback_radius && offset2 > ctx.fallback_radius {
        // Finished in front
        return true;
    }

    let triangle = [a, b, c];
    let input = TOIInput {
        proxy_a: ShapeProxy { points: &triangle, radius: 0.0 },
        proxy_b: ctx.proxy_b,
        sweep_a: ctx.sweep_a,
        sweep_b: ctx.sweep_b,
        max_fraction: ctx.max_fraction,
    };

    let output = crate::distance::time_of_impact(&input);

    // It is possible for a hit at fraction == 0

    if 0.0 < output.fraction && output.fraction < ctx.max_fraction {
        ctx.toi_output = output;
        ctx.max_fraction = output.fraction;
    } else if 0.0 == output.fraction {
        // fallback to TOI of a small circle around the fast shape centroid
        let fallback_input = TOIInput {
            proxy_a: ShapeProxy { points: &triangle, radius: 0.0 },
            proxy_b: ShapeProxy {
                points: std::slice::from_ref(&ctx.local_centroid_b),
                radius: ctx.fallback_radius + crate::constants::linear_slop(),
            },
            sweep_a: ctx.sweep_a,
            sweep_b: ctx.sweep_b,
            max_fraction: ctx.max_fraction,
        };
        let output = crate::distance::time_of_impact(&fallback_input);

        if 0.0 < output.fraction && output.fraction < ctx.max_fraction {
            ctx.toi_output = output;
            ctx.max_fraction = output.fraction;
            ctx.toi_output.used_fallback = true;
        }
    }

    // Continue the query
    true
}

pub fn shape_time_of_impact(shape_a: &Shape, shape_b: &Shape, sweep_a: &Sweep, sweep_b: &Sweep, max_fraction: f32) -> TOIOutput {
    let is_sensor = shape_a.sensor_index != NULL_INDEX;

    let mut buffer_b = [Vec3::ZERO; 2];

    match &shape_a.geom {
        ShapeGeometry::Compound(compound) => {
            // todo implement b3CompoundTimeOfImpact
            let proxy_b = make_shape_proxy(shape_b, &mut buffer_b);

            let compound_transform = Transform { p: sweep_a.c1, q: sweep_a.q1 };

            let local_centroid_b = get_shape_centroid(shape_b);

            let extents = compute_shape_extent(shape_b, local_centroid_b);
            let fallback_radius = max_float(0.75 * extents.min_extent, speculative_distance());

            // Swept bounds of shapeB
            let bounds = compute_swept_shape_aabb(shape_b, sweep_b, max_fraction);

            // Bounds local to compound
            let local_sweep_bounds_b = crate::math_functions::aabb_transform(invert_transform(compound_transform), bounds);

            let mut toi_output = TOIOutput::default();
            let mut cur_max_fraction = max_fraction;

            crate::compound::query_compound(compound, local_sweep_bounds_b, &mut |compound_ref, child_index| {
                let child = crate::compound::get_compound_child(compound_ref, child_index);
                let child_sweep_a = crate::compound::make_compound_child_sweep(compound_transform, child.transform);

                let output = match &child.geom {
                    crate::types::ChildShapeGeom::Capsule(capsule) => {
                        let points = [capsule.center1, capsule.center2];
                        let input = TOIInput {
                            proxy_a: ShapeProxy { points: &points, radius: capsule.radius },
                            proxy_b,
                            sweep_a: child_sweep_a,
                            sweep_b: *sweep_b,
                            max_fraction: cur_max_fraction,
                        };
                        crate::distance::time_of_impact(&input)
                    }

                    crate::types::ChildShapeGeom::Hull(hull) => {
                        let input = TOIInput {
                            proxy_a: ShapeProxy { points: &hull.points, radius: 0.0 },
                            proxy_b,
                            sweep_a: child_sweep_a,
                            sweep_b: *sweep_b,
                            max_fraction: cur_max_fraction,
                        };
                        crate::distance::time_of_impact(&input)
                    }

                    crate::types::ChildShapeGeom::Mesh(child_mesh) => {
                        let mesh_world_transform = mul_transforms(compound_transform, child.transform);

                        let xf_b1 = Transform {
                            p: sub(sweep_b.c1, rotate_vector(sweep_b.q1, sweep_b.local_center)),
                            q: sweep_b.q1,
                        };

                        let xf_b2 = Transform {
                            p: sub(sweep_b.c2, rotate_vector(sweep_b.q2, sweep_b.local_center)),
                            q: sweep_b.q2,
                        };

                        let mut mesh_ctx = MeshImpactContext {
                            proxy_b,
                            sweep_a: child_sweep_a,
                            sweep_b: *sweep_b,
                            max_fraction: cur_max_fraction,
                            toi_output: TOIOutput::default(),
                            local_centroid_b,
                            mesh_local_centroid_b1: inv_transform_point(
                                mesh_world_transform,
                                transform_point(xf_b1, local_centroid_b),
                            ),
                            mesh_local_centroid_b2: inv_transform_point(
                                mesh_world_transform,
                                transform_point(xf_b2, local_centroid_b),
                            ),
                            fallback_radius,
                            is_sensor: false,
                            visit_count: 0,
                        };

                        // Bounds local to mesh
                        let local_bounds = crate::math_functions::aabb_transform(
                            invert_transform(child.transform),
                            local_sweep_bounds_b,
                        );

                        crate::mesh::query_mesh(child_mesh, local_bounds, &mut |a, b, c, tri| {
                            mesh_time_of_impact_fcn(&mut mesh_ctx, a, b, c, tri)
                        });

                        mesh_ctx.toi_output
                    }

                    crate::types::ChildShapeGeom::Sphere(sphere) => {
                        let input = TOIInput {
                            proxy_a: ShapeProxy { points: std::slice::from_ref(&sphere.center), radius: sphere.radius },
                            proxy_b,
                            sweep_a: child_sweep_a,
                            sweep_b: *sweep_b,
                            max_fraction: cur_max_fraction,
                        };
                        crate::distance::time_of_impact(&input)
                    }
                };

                if 0.0 < output.fraction && output.fraction < cur_max_fraction {
                    toi_output = output;
                    cur_max_fraction = output.fraction;
                }

                // Continue the query
                true
            });

            return toi_output;
        }

        ShapeGeometry::Mesh(_) | ShapeGeometry::HeightField(_) => {
            // todo implement b3MeshTimeOfImpact and b3HeightFieldTimeOfImpact
            // Note: assuming mesh is static

            let proxy_b = make_shape_proxy(shape_b, &mut buffer_b);

            let local_centroid_b = get_shape_centroid(shape_b);

            // Assume mesh is static
            let xf_a = Transform {
                p: sub(sweep_a.c1, rotate_vector(sweep_a.q1, sweep_a.local_center)),
                q: sweep_a.q1,
            };

            let xf_b1 = Transform {
                p: sub(sweep_b.c1, rotate_vector(sweep_b.q1, sweep_b.local_center)),
                q: sweep_b.q1,
            };

            let xf_b2 = Transform {
                p: sub(sweep_b.c2, rotate_vector(sweep_b.q2, sweep_b.local_center)),
                q: sweep_b.q2,
            };

            let extents = compute_shape_extent(shape_b, local_centroid_b);
            let fallback_radius = max_float(0.5 * extents.min_extent, crate::constants::linear_slop());

            let mut ctx = MeshImpactContext {
                proxy_b,
                sweep_a: *sweep_a,
                sweep_b: *sweep_b,
                max_fraction,
                toi_output: TOIOutput::default(),
                local_centroid_b,
                mesh_local_centroid_b1: inv_transform_point(xf_a, transform_point(xf_b1, local_centroid_b)),
                mesh_local_centroid_b2: inv_transform_point(xf_a, transform_point(xf_b2, local_centroid_b)),
                fallback_radius,
                is_sensor,
                visit_count: 0,
            };

            // Swept bounds of shapeB
            // todo pass in xfA to get local bounds directly
            let bounds = compute_swept_shape_aabb(shape_b, sweep_b, max_fraction);

            // Bounds local to mesh
            let local_bounds = crate::math_functions::aabb_transform(invert_transform(xf_a), bounds);

            match &shape_a.geom {
                ShapeGeometry::Mesh(mesh) => {
                    crate::mesh::query_mesh(mesh, local_bounds, &mut |a, b, c, tri| {
                        mesh_time_of_impact_fcn(&mut ctx, a, b, c, tri)
                    });
                }
                ShapeGeometry::HeightField(height_field) => {
                    crate::height_field::query_height_field(height_field, local_bounds, &mut |a, b, c, tri| {
                        mesh_time_of_impact_fcn(&mut ctx, a, b, c, tri)
                    });
                }
                _ => unreachable!(),
            }

            // CCD stall logging (b3GetStallThreshold) not ported.

            return ctx.toi_output;
        }

        _ => {}
    }

    b3_assert!(
        shape_b.shape_type() != ShapeType::Compound
            && shape_b.shape_type() != ShapeType::Mesh
            && shape_b.shape_type() != ShapeType::Height
    );

    let mut buffer_a = [Vec3::ZERO; 2];
    let input = TOIInput {
        proxy_a: make_shape_proxy(shape_a, &mut buffer_a),
        proxy_b: make_shape_proxy(shape_b, &mut buffer_b),
        sweep_a: *sweep_a,
        sweep_b: *sweep_b,
        max_fraction,
    };

    crate::distance::time_of_impact(&input)
}
