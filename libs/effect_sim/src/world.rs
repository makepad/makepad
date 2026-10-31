//! The box3d side: one world per stepper, bodies made from validated
//! records, poses read back, hits and sleeps collected after each step,
//! and the world's snapshot image with its geometry registry (restored
//! into a shell world exactly as box3d's `tests/test_snapshot.rs` does).

use crate::desc::{PhysicsDesc, Shape};
use crate::layouts::{BodyRecord, RecordShape};
use makepad_box3d::body::*;
use makepad_box3d::hull::{create_hull, make_box_hull};
use makepad_box3d::id::BodyId;
use makepad_box3d::math_functions::{pos, Pos, Quat, Vec3, WorldTransform};
use makepad_box3d::mesh::create_mesh;
use makepad_box3d::physics_world::*;
use makepad_box3d::recording::{write_registry, RecBuffer, Recording};
use makepad_box3d::recording_replay::load_registry;
use makepad_box3d::shape::*;
use makepad_box3d::types::*;
use makepad_box3d::world_snapshot::{deserialize_into_shell, serialize_world};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn v3(a: [f32; 3]) -> Vec3 {
    Vec3 { x: a[0], y: a[1], z: a[2] }
}

/// A world position from a record. box3d's `Pos` is `Vec3` in single
/// precision and its own f64 struct with `double-precision`, which another
/// crate in the same build (the sandbox's sim) turns on; `pos` and `f32s`
/// read the same in both.
fn p3(a: [f32; 3]) -> Pos {
    pos(a[0], a[1], a[2])
}

#[allow(clippy::unnecessary_cast)]
fn f32s(p: Pos) -> [f32; 3] {
    [p.x as f32, p.y as f32, p.z as f32]
}

fn world_def(desc: &PhysicsDesc) -> WorldDef {
    let mut def = default_world_def();
    def.gravity = v3(desc.gravity);
    def.hit_event_threshold = desc.hit_speed.max(0.0);
    def.enable_sleep = desc.sleep;
    // Serial: an effect world is one job on one worker; box3d gives the
    // same result on any worker count, the stepper just never fans out.
    def.worker_count = 1;
    def
}

pub(crate) struct PhysWorld {
    pub world: World,
    /// Box hulls by exact half extents (shared geometry).
    hulls: HashMap<[u32; 3], Arc<makepad_box3d::types::HullData>>,
}

// SAFETY: box3d's World is not Send only because it can hold a user task
// system (`*mut ()` context, callbacks) and custom filter / pre-solve
// callbacks. An effect world is made by `world_def` with the serial task
// system (worker_count 1, no task callbacks) and never gets a filter or
// pre-solve callback, so it holds no pointer or closure tied to a thread;
// moving it to another thread (the stepper runs on a worker) is sound. It
// is never shared between threads (PhysWorld is not Sync).
unsafe impl Send for PhysWorld {}

/// A collider's rough size: the radius of its bounding sphere about the
/// body origin (queries and culling).
pub(crate) fn bounding_radius(shape: &Shape, size: [f32; 3]) -> f32 {
    let h = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    match shape {
        Shape::Box | Shape::Plane => (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt(),
        Shape::Sphere => h[0].max(h[1]).max(h[2]),
        Shape::Capsule => h[1].max(h[0]),
        Shape::Hull(p) | Shape::Mesh { vertices: p, .. } => p.iter().map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()).fold(0.0, f32::max),
        Shape::HeightField { count_x, count_z, cell, .. } => {
            let (x, z) = (*count_x as f32 * cell[0], *count_z as f32 * cell[1]);
            (x * x + z * z).sqrt()
        }
    }
}

/// How a body is made.
pub(crate) enum Kind {
    Fixed,
    Dynamic,
    Kinematic,
}

impl PhysWorld {
    pub fn new(desc: &PhysicsDesc) -> Self {
        Self { world: create_world(&world_def(desc)), hulls: HashMap::new() }
    }

    /// Creates a body from a validated record; `None` when box3d refuses
    /// the geometry (a degenerate hull or mesh), with why.
    pub fn create(&mut self, desc: &PhysicsDesc, shape: &Shape, kind: Kind, size: [f32; 3], rec: &BodyRecord) -> Result<BodyId, String> {
        let mut bd = default_body_def();
        bd.body_type = match kind {
            Kind::Fixed => BodyType::Static,
            Kind::Dynamic => BodyType::Dynamic,
            Kind::Kinematic => BodyType::Kinematic,
        };
        let q = rec.rotation();
        bd.position = p3(rec.pos);
        bd.rotation = Quat { v: Vec3 { x: q[0], y: q[1], z: q[2] }, s: q[3] };
        if matches!(kind, Kind::Dynamic) {
            bd.linear_velocity = v3(rec.vel);
            bd.angular_velocity = v3(rec.spin);
            bd.linear_damping = rec.drag;
            bd.angular_damping = rec.spin_drag;
            bd.gravity_scale = 1.0 - rec.lift;
        }
        bd.enable_sleep = desc.sleep;
        let mut sd = default_shape_def();
        if rec.density > 0.0 {
            sd.density = rec.density;
        }
        if rec.friction > 0.0 {
            sd.base_material.friction = rec.friction;
        }
        sd.base_material.restitution = rec.bounce;
        sd.enable_hit_events = !matches!(kind, Kind::Fixed);
        let shape = match (shape, rec.record_shape()) {
            (_, RecordShape::Box) => &Shape::Box,
            (_, RecordShape::Sphere) => &Shape::Sphere,
            (_, RecordShape::Capsule) => &Shape::Capsule,
            (s, RecordShape::Spawn) => s,
        };
        let h = [(size[0] * 0.5).max(0.0005), (size[1] * 0.5).max(0.0005), (size[2] * 0.5).max(0.0005)];
        // Geometry first, so a refused shape creates no body.
        enum Geo {
            Hull(Arc<makepad_box3d::types::HullData>),
            Sphere(f32),
            Capsule(f32, f32),
            Mesh(Arc<makepad_box3d::types::MeshData>),
            Height(Arc<makepad_box3d::types::HeightFieldData>),
        }
        let geo = match shape {
            Shape::Box => Geo::Hull(self.box_hull(h)),
            Shape::Plane => {
                // A slab 2000 across and 1 thick, its top at the body's y.
                bd.position.y -= 0.5;
                Geo::Hull(self.box_hull([1000.0, 0.5, 1000.0]))
            }
            Shape::Sphere => Geo::Sphere(h[0].max(h[1]).max(h[2])),
            Shape::Capsule => {
                let r = h[0].max(h[2]);
                Geo::Capsule(r, (h[1] - r).max(0.0))
            }
            Shape::Hull(points) => {
                let pts: Vec<Vec3> = points.iter().map(|p| v3(*p)).collect();
                Geo::Hull(create_hull(&pts, 64).ok_or("the hull's points are degenerate (flat, too few or coincident)")?)
            }
            Shape::Mesh { vertices, indices } => {
                if indices.len() < 3 || indices.len() % 3 != 0 || indices.iter().any(|i| *i as usize >= vertices.len()) {
                    return Err(format!("a mesh collider needs whole triangles indexing its {} vertices", vertices.len()));
                }
                let verts: Vec<Vec3> = vertices.iter().map(|p| v3(*p)).collect();
                let idx: Vec<i32> = indices.iter().map(|i| *i as i32).collect();
                let def = MeshDef { vertices: &verts, indices: &idx, weld_vertices: true, weld_tolerance: 1e-4, identify_edges: true, ..Default::default() };
                Geo::Mesh(create_mesh(&def, None).ok_or("the mesh collider has no valid triangles")?)
            }
            Shape::HeightField { heights, count_x, count_z, cell } => {
                let (nx, nz) = (*count_x as usize, *count_z as usize);
                if nx < 2 || nz < 2 || heights.len() != nx * nz || !(cell[0] > 0.0 && cell[1] > 0.0) {
                    return Err(format!("a height field of {nx}×{nz} needs {} heights and a positive cell", nx * nz));
                }
                let (lo, hi) = heights.iter().fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
                let def = HeightFieldDef {
                    heights,
                    material_indices: &[],
                    scale: Vec3 { x: cell[0], y: 1.0, z: cell[1] },
                    count_x: nx as i32,
                    count_z: nz as i32,
                    global_minimum_height: lo,
                    global_maximum_height: hi.max(lo + 1e-3),
                    ..Default::default()
                };
                Geo::Height(makepad_box3d::height_field::create_height_field(&def))
            }
        };
        let body = create_body(&mut self.world, &bd);
        match geo {
            Geo::Hull(hull) => {
                create_hull_shape(&mut self.world, body, &sd, &hull);
            }
            Geo::Sphere(r) => {
                create_sphere_shape(&mut self.world, body, &sd, &Sphere { center: Vec3::ZERO, radius: r });
            }
            Geo::Capsule(r, half) => {
                let cap = Capsule { center1: Vec3 { x: 0.0, y: -half, z: 0.0 }, center2: Vec3 { x: 0.0, y: half, z: 0.0 }, radius: r };
                create_capsule_shape(&mut self.world, body, &sd, &cap);
            }
            Geo::Mesh(mesh) => {
                create_mesh_shape(&mut self.world, body, &sd, &mesh, Vec3 { x: 1.0, y: 1.0, z: 1.0 });
            }
            Geo::Height(hf) => {
                create_height_field_shape(&mut self.world, body, &sd, &hf);
            }
        }
        Ok(body)
    }

    fn box_hull(&mut self, h: [f32; 3]) -> Arc<makepad_box3d::types::HullData> {
        let key = [h[0].to_bits(), h[1].to_bits(), h[2].to_bits()];
        self.hulls.entry(key).or_insert_with(|| make_box_hull(h[0], h[1], h[2])).clone()
    }

    pub fn destroy(&mut self, body: BodyId) {
        if body_is_valid(&self.world, body) {
            destroy_body(&mut self.world, body);
        }
    }

    /// Position and rotation (x, y, z, w) of a body.
    pub fn pose(&self, body: BodyId) -> [f32; 7] {
        let p = body_get_position(&self.world, body);
        let q = body_get_rotation(&self.world, body);
        let [x, y, z] = f32s(p);
        [x, y, z, q.v.x, q.v.y, q.v.z, q.s]
    }

    pub fn speed(&self, body: BodyId) -> f32 {
        let v = body_get_linear_velocity(&self.world, body);
        (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
    }

    /// Words that fully describe a body's motion state (pose and both
    /// velocities), for the state hash.
    pub fn motion_words(&self, body: BodyId, out: &mut Vec<u32>) {
        let pose = self.pose(body);
        let v = body_get_linear_velocity(&self.world, body);
        let w = body_get_angular_velocity(&self.world, body);
        out.extend(pose.iter().map(|x| x.to_bits()));
        out.extend([v.x, v.y, v.z, w.x, w.y, w.z].iter().map(|x| x.to_bits()));
    }

    pub fn set_target(&mut self, body: BodyId, pose: ([f32; 3], [f32; 4]), dt: f32) {
        let q = pose.1;
        let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        let q = if l > 1e-6 && l.is_finite() { [q[0] / l, q[1] / l, q[2] / l, q[3] / l] } else { [0.0, 0.0, 0.0, 1.0] };
        if !pose.0.iter().all(|x| x.is_finite() && x.abs() < 1.0e5) {
            return;
        }
        let target = WorldTransform { p: p3(pose.0), q: Quat { v: Vec3 { x: q[0], y: q[1], z: q[2] }, s: q[3] } };
        body_set_target_transform(&mut self.world, body, target, dt, true);
    }

    pub fn step(&mut self, dt: f32, substeps: u32) {
        world_step(&mut self.world, dt, substeps.clamp(1, 16) as i32);
    }

    /// The step's hits: (point, normal, approach speed, body a, b, reduced
    /// mass), in box3d's event order. (Bodies, not user data: a snapshot
    /// scrubs user data, the stepper maps body ids to its stable ids.)
    pub fn hits(&self, out: &mut Vec<([f32; 3], [f32; 3], f32, BodyId, BodyId, f32)>) {
        let ev = world_get_contact_events(&self.world);
        for h in ev.hit_events {
            if !shape_is_valid(&self.world, h.shape_id_a) || !shape_is_valid(&self.world, h.shape_id_b) {
                continue;
            }
            let (ba, bb) = (shape_get_body(&self.world, h.shape_id_a), shape_get_body(&self.world, h.shape_id_b));
            let (ma, mb) = (body_get_mass(&self.world, ba), body_get_mass(&self.world, bb));
            let m = if ma > 0.0 && mb > 0.0 { ma * mb / (ma + mb) } else { ma.max(mb) };
            out.push((f32s(h.point), [h.normal.x, h.normal.y, h.normal.z], h.approach_speed, ba, bb, m));
        }
    }

    /// The bodies that fell asleep this step.
    pub fn slept(&self, out: &mut Vec<BodyId>) {
        for e in world_get_body_events(&self.world).move_events {
            if e.fell_asleep {
                out.push(e.body_id);
            }
        }
    }

    /// The world's snapshot image and its geometry registry.
    pub fn save(&self) -> (Vec<u8>, Vec<u8>) {
        let mut buf = RecBuffer::new();
        let mut rec = Recording::new();
        serialize_world(&self.world, &mut buf, &mut rec);
        write_registry(&mut rec);
        (buf.data, rec.buffer.data)
    }

    /// A world restored from [`Self::save`]'s bytes into a fresh shell.
    pub fn restore(desc: &PhysicsDesc, image: &[u8], registry: &[u8]) -> Option<Self> {
        let rdr = load_registry(registry)?;
        let mut world = create_world(&world_def(desc));
        if !deserialize_into_shell(image, &mut world, &rdr) {
            return None;
        }
        Some(Self { world, hulls: HashMap::new() })
    }
}
