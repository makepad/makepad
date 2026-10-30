//! The assembled piece in box3d: each printed part a rigid body (mass and
//! inertia from its mesh, collision hulls away from its joints), each joint a
//! box3d joint with its limits and load-dependent pin friction, and a held
//! ground for anchors. A millimetre / gram / second world: gravity 9810, so
//! box3d's contact slop is 5 um, well under a 0.3 mm clearance.
//!
//! [`MechWorld`] is the live world (the Mechanics view steps it and grabs
//! parts); [`simulate`] runs a scenario headless and answers in numbers, the
//! same every run (fixed step, one solver thread, seeded shake).

use crate::geom::*;
use crate::mass::MassProps;
use makepad_box3d::body::*;
use makepad_box3d::hull::create_hull;
use makepad_box3d::id::{BodyId, JointId};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{make_quat_from_axis_angle, to_pos, to_vec3, vec3, Matrix3, Quat, Transform, Vec3, WorldTransform};
use makepad_box3d::physics_world::*;
use makepad_box3d::revolute_joint::*;
use makepad_box3d::shape::{create_hull_shape, shape_get_body};
use makepad_box3d::spherical_joint::*;
use makepad_box3d::types::*;

pub const GRAVITY: f64 = 9810.0;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Friction {
    pub mu: f64,
    /// Bearing radius, mm.
    pub radius: f64,
    /// Load-free torque (stiction, preload), g mm^2 / s^2.
    pub torque: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BodySpec {
    pub name: String,
    pub mass: MassProps,
    /// Convex point clouds (model frame) for collision.
    pub hulls: Vec<Vec<P3>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JointKind {
    /// About `axis`; limits radians (B relative to A).
    Revolute { lower: f64, upper: f64 },
    /// Cone about `axis` (the opening direction); twist about the neck.
    Spherical { cone: f64, twist: Option<(f64, f64)> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct JointSpec {
    pub name: String,
    pub a: usize,
    pub b: usize,
    pub pivot: P3,
    pub axis: P3,
    pub kind: JointKind,
    pub friction: Friction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnchorKind {
    /// Held fixed (welded to the ground).
    Fixed,
    /// Hangs on a pin through `pivot` along the axis.
    Pin(P3),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnchorSpec {
    pub body: usize,
    pub pivot: P3,
    pub kind: AnchorKind,
    /// A hanging pin's friction.
    pub friction: Friction,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Assembly {
    pub bodies: Vec<BodySpec>,
    pub joints: Vec<JointSpec>,
    pub anchors: Vec<AnchorSpec>,
    /// Print up (model frame).
    pub up: P3,
}

impl Assembly {
    pub fn centroid(&self) -> P3 {
        let (mut s, mut m) = ([0.0; 3], 0.0);
        for b in &self.bodies {
            s = madd(s, b.mass.com, b.mass.mass);
            m += b.mass.mass;
        }
        if m > 0.0 {
            scale(s, 1.0 / m)
        } else {
            [0.0; 3]
        }
    }

    /// The gravity of a held hanging test. Hung on a pin: across the print up and across the line from the pin
    /// to the centroid, so the piece swings down from how it was printed. Held fixed: from the held part's
    /// centre of mass toward the rest (it dangles from the held part). Straight down (-up) without an anchor.
    pub fn hang_gravity(&self) -> P3 {
        let up = norm(self.up);
        let Some(anchor) = self.anchors.first() else { return scale(up, -1.0) };
        if anchor.kind == AnchorKind::Fixed {
            let held: Vec<usize> = self.anchors.iter().map(|a| a.body).collect();
            let (mut m, mut c) = (0.0, [0.0; 3]);
            for (i, b) in self.bodies.iter().enumerate() {
                if !held.contains(&i) {
                    m += b.mass.mass;
                    c = madd(c, b.mass.com, b.mass.mass);
                }
            }
            if m > 0.0 {
                let d = sub(scale(c, 1.0 / m), self.bodies[anchor.body].mass.com);
                if len(d) > 1e-6 {
                    return norm(d);
                }
            }
            return scale(up, -1.0);
        }
        let d = sub(self.centroid(), anchor.pivot);
        let side = cross(up, d);
        if len(side) < 1e-6 * len(d).max(1.0) {
            return scale(up, -1.0);
        }
        // in the bed plane, across the arm: the piece swings down about its pins
        norm(side)
    }
}

fn vf(p: P3) -> Vec3 {
    vec3(p[0] as f32, p[1] as f32, p[2] as f32)
}
fn pf(v: Vec3) -> P3 {
    [v.x as f64, v.y as f64, v.z as f64]
}

/// A quaternion turning +Z onto `axis`.
fn quat_z_to(axis: P3) -> Quat {
    let a = norm(axis);
    let z = [0.0, 0.0, 1.0];
    let c = dot(z, a);
    if c > 1.0 - 1e-9 {
        return Quat::IDENTITY;
    }
    if c < -1.0 + 1e-9 {
        return make_quat_from_axis_angle(vec3(1.0, 0.0, 0.0), std::f32::consts::PI);
    }
    let k = norm(cross(z, a));
    make_quat_from_axis_angle(vf(k), c.clamp(-1.0, 1.0).acos() as f32)
}

fn quat_to_euler_deg(q: Quat) -> P3 {
    let (x, y, z, w) = (q.v.x as f64, q.v.y as f64, q.v.z as f64, q.s as f64);
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    euler_xyz(&r)
}

/// A body's pose: centre of mass position and rotation (x, y, z, w).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub com: P3,
    pub rot: [f64; 4],
}

impl Pose {
    /// Model-frame point `p` of the body (as built) where it is now.
    pub fn apply(&self, rest_com: P3, p: P3) -> P3 {
        let q = Quat { v: vec3(self.rot[0] as f32, self.rot[1] as f32, self.rot[2] as f32), s: self.rot[3] as f32 };
        add(self.com, pf(makepad_box3d::math_functions::rotate_vector(q, vf(sub(p, rest_com)))))
    }
}

struct Grab {
    target: BodyId,
    joint: JointId,
    body: usize,
}

/// A live box3d world of an assembly.
pub struct MechWorld {
    pub world: World,
    pub ground: BodyId,
    pub bodies: Vec<BodyId>,
    pub joints: Vec<JointId>,
    rest: Vec<P3>,
    grab: Option<Grab>,
    floor: Option<BodyId>,
    pub gravity: P3,
    time: f64,
    ground_pose: (P3, f64),
}

impl MechWorld {
    pub fn new(asm: &Assembly, gravity: P3, floor: Option<f64>) -> MechWorld {
        let mut def = default_world_def();
        def.gravity = vf(gravity);
        def.maximum_linear_speed = 400_000.0;
        def.contact_speed = 3000.0;
        def.worker_count = 1;
        let mut world = create_world(&def);
        let mut gd = default_body_def();
        gd.body_type = BodyType::Kinematic;
        let ground = create_body(&mut world, &gd);
        let mut bodies = Vec::new();
        let mut rest = Vec::new();
        for (i, b) in asm.bodies.iter().enumerate() {
            let mut bd = default_body_def();
            bd.body_type = BodyType::Dynamic;
            bd.position = to_pos(vf(b.mass.com));
            bd.enable_sleep = false;
            let id = create_body(&mut world, &bd);
            let mut sd = default_shape_def();
            sd.base_material.friction = 0.35;
            sd.enable_contact_events = true;
            sd.enable_hit_events = true;
            sd.update_body_mass = false;
            sd.user_data = i as u64;
            for h in &b.hulls {
                let pts: Vec<Vec3> = h.iter().map(|p| vf(sub(*p, b.mass.com))).collect();
                if let Some(hull) = create_hull(&pts, 64) {
                    create_hull_shape(&mut world, id, &sd, &hull);
                }
            }
            let m = &b.mass;
            let inertia = Matrix3 {
                cx: vec3(m.inertia[0][0] as f32, m.inertia[1][0] as f32, m.inertia[2][0] as f32),
                cy: vec3(m.inertia[0][1] as f32, m.inertia[1][1] as f32, m.inertia[2][1] as f32),
                cz: vec3(m.inertia[0][2] as f32, m.inertia[1][2] as f32, m.inertia[2][2] as f32),
            };
            body_set_mass_data(&mut world, id, MassData { mass: m.mass.max(1e-6) as f32, center: Vec3::ZERO, inertia });
            bodies.push(id);
            rest.push(b.mass.com);
        }
        let local = |rest: &[P3], i: usize, p: P3| vf(sub(p, rest[i]));
        let mut joints = Vec::new();
        for j in &asm.joints {
            let q = quat_z_to(j.axis);
            let base = JointDefBase { a: bodies[j.a], b: bodies[j.b], fa: Transform { p: local(&rest, j.a, j.pivot), q }, fb: Transform { p: local(&rest, j.b, j.pivot), q } };
            let friction = JointFriction { coefficient: j.friction.mu as f32, radius: j.friction.radius as f32, torque: j.friction.torque as f32 };
            let id = match &j.kind {
                JointKind::Revolute { lower, upper } => {
                    let mut d = default_revolute_joint_def();
                    base.fill(&mut d.base);
                    d.base.friction = friction;
                    d.enable_limit = true;
                    d.lower_angle = *lower as f32;
                    d.upper_angle = *upper as f32;
                    create_revolute_joint(&mut world, &d)
                }
                JointKind::Spherical { cone, twist } => {
                    let mut d = default_spherical_joint_def();
                    base.fill(&mut d.base);
                    d.base.friction = friction;
                    d.enable_cone_limit = true;
                    d.cone_angle = *cone as f32;
                    if let Some((lo, hi)) = twist {
                        d.enable_twist_limit = true;
                        d.lower_twist_angle = *lo as f32;
                        d.upper_twist_angle = *hi as f32;
                    }
                    create_spherical_joint(&mut world, &d)
                }
            };
            joints.push(id);
        }
        for a in &asm.anchors {
            match &a.kind {
                AnchorKind::Fixed => {
                    let mut d = default_weld_joint_def();
                    d.base.body_id_a = ground;
                    d.base.body_id_b = bodies[a.body];
                    d.base.local_frame_a = Transform { p: vf(a.pivot), q: Quat::IDENTITY };
                    d.base.local_frame_b = Transform { p: local(&rest, a.body, a.pivot), q: Quat::IDENTITY };
                    makepad_box3d::joint::create_weld_joint(&mut world, &d);
                }
                AnchorKind::Pin(axis) => {
                    let q = quat_z_to(*axis);
                    let mut d = default_revolute_joint_def();
                    d.base.body_id_a = ground;
                    d.base.body_id_b = bodies[a.body];
                    d.base.local_frame_a = Transform { p: vf(a.pivot), q };
                    d.base.local_frame_b = Transform { p: local(&rest, a.body, a.pivot), q };
                    d.base.friction = JointFriction { coefficient: a.friction.mu as f32, radius: a.friction.radius as f32, torque: a.friction.torque as f32 };
                    create_revolute_joint(&mut world, &d);
                }
            }
        }
        let floor_id = floor.map(|level| {
            let g = norm(gravity);
            let mut bd = default_body_def();
            bd.position = to_pos(vf(scale(g, -level + 5.0)));
            bd.rotation = quat_z_to(scale(g, -1.0));
            let id = create_body(&mut world, &bd);
            let mut sd = default_shape_def();
            sd.base_material.friction = 0.4;
            sd.user_data = u64::MAX;
            create_hull_shape(&mut world, id, &sd, &makepad_box3d::hull::make_box_hull(2000.0, 2000.0, 5.0));
            id
        });
        MechWorld { world, ground, bodies, joints, rest, grab: None, floor: floor_id, gravity, time: 0.0, ground_pose: ([0.0; 3], 0.0) }
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    /// One fixed step (`dt` seconds, 4 substeps).
    pub fn step(&mut self, dt: f64) {
        world_step(&mut self.world, dt as f32, 4);
        self.time += dt;
    }

    pub fn poses(&self) -> Vec<Pose> {
        self.bodies
            .iter()
            .map(|b| {
                let t = body_get_transform(&self.world, *b);
                Pose { com: pf(to_vec3(t.p)), rot: [t.q.v.x as f64, t.q.v.y as f64, t.q.v.z as f64, t.q.s as f64] }
            })
            .collect()
    }

    pub fn rest_com(&self, body: usize) -> P3 {
        self.rest[body]
    }

    /// Joint angles: revolute angle, or a spherical joint's cone angle (radians).
    pub fn angles(&mut self, asm: &Assembly) -> Vec<f64> {
        asm.joints
            .iter()
            .zip(self.joints.clone())
            .map(|(j, id)| match j.kind {
                JointKind::Revolute { .. } => revolute_joint_get_angle(&mut self.world, id) as f64,
                JointKind::Spherical { .. } => spherical_joint_get_cone_angle(&mut self.world, id) as f64,
            })
            .collect()
    }

    /// Move the held ground (anchors): a translation and a turn about `axis` through the origin.
    pub fn move_ground(&mut self, at: P3, axis: P3, ang: f64, dt: f64) {
        let q = make_quat_from_axis_angle(vf(norm(axis)), ang as f32);
        body_set_target_transform(&mut self.world, self.ground, WorldTransform { p: to_pos(vf(at)), q }, dt as f32, true);
        self.ground_pose = (at, ang);
    }

    /// Start dragging `body` by the model point `point` (where it is now).
    pub fn grab(&mut self, body: usize, point: P3, stiffness_hz: f64) {
        self.release();
        let mut bd = default_body_def();
        bd.body_type = BodyType::Kinematic;
        bd.position = to_pos(vf(point));
        let target = create_body(&mut self.world, &bd);
        let b = self.bodies[body];
        let local_b = body_get_local_point(&self.world, b, to_pos(vf(point)));
        let mass = body_get_mass(&self.world, b) as f64;
        let mut d = default_motor_joint_def();
        d.base.body_id_a = target;
        d.base.body_id_b = b;
        d.base.local_frame_a = Transform::IDENTITY;
        d.base.local_frame_b = Transform { p: local_b, q: Quat::IDENTITY };
        d.linear_hertz = stiffness_hz as f32;
        d.linear_damping_ratio = 0.7;
        d.max_spring_force = (mass * GRAVITY * 200.0) as f32;
        d.angular_hertz = 0.0;
        d.max_spring_torque = 0.0;
        d.max_velocity_force = 0.0;
        d.max_velocity_torque = 0.0;
        let joint = create_motor_joint(&mut self.world, &d);
        self.grab = Some(Grab { target, joint, body });
    }

    /// Move the grab target (model point); the part follows through the spring.
    pub fn drag(&mut self, point: P3, dt: f64) {
        if let Some(g) = &self.grab {
            body_set_target_transform(&mut self.world, g.target, WorldTransform { p: to_pos(vf(point)), q: Quat::IDENTITY }, dt as f32, true);
        }
    }

    /// Let go (the part keeps its velocity: a throw).
    pub fn release(&mut self) {
        if let Some(g) = self.grab.take() {
            destroy_joint(&mut self.world, g.joint, true);
            destroy_body(&mut self.world, g.target);
        }
    }

    pub fn grabbed(&self) -> Option<usize> {
        self.grab.as_ref().map(|g| g.body)
    }

    /// Contacts that began this step: (body, body or None for the floor, approach speed).
    pub fn contact_begins(&self) -> Vec<(usize, Option<usize>, f64)> {
        let ev = world_get_contact_events(&self.world);
        let mut out = Vec::new();
        for e in ev.begin_events {
            let ia = self.body_index(shape_get_body(&self.world, e.shape_id_a));
            let ib = self.body_index(shape_get_body(&self.world, e.shape_id_b));
            match (ia, ib) {
                (Some(a), Some(b)) => out.push((a.min(b), Some(a.max(b)), 0.0)),
                (Some(a), None) | (None, Some(a)) => out.push((a, None, 0.0)),
                _ => {}
            }
        }
        out
    }

    /// Hard contacts this step (for haptics and sounds): (body, other body or None for the floor, approach speed mm/s).
    pub fn hits(&self) -> Vec<(usize, Option<usize>, f64)> {
        let ev = world_get_contact_events(&self.world);
        let mut out = Vec::new();
        for e in ev.hit_events {
            let ia = self.body_index(shape_get_body(&self.world, e.shape_id_a));
            let ib = self.body_index(shape_get_body(&self.world, e.shape_id_b));
            match (ia, ib) {
                (Some(a), b) => out.push((a, b, e.approach_speed as f64)),
                (None, Some(b)) => out.push((b, None, e.approach_speed as f64)),
                _ => {}
            }
        }
        out
    }

    fn body_index(&self, id: BodyId) -> Option<usize> {
        if Some(id) == self.floor {
            return None;
        }
        self.bodies.iter().position(|b| *b == id)
    }

    pub fn max_angular_speed(&self) -> f64 {
        self.bodies
            .iter()
            .map(|b| {
                let w = body_get_angular_velocity(&self.world, *b);
                ((w.x * w.x + w.y * w.y + w.z * w.z) as f64).sqrt()
            })
            .fold(0.0, f64::max)
    }

    pub fn joint_force(&self, j: usize) -> f64 {
        let f = joint_get_constraint_force(&self.world, self.joints[j]);
        ((f.x * f.x + f.y * f.y + f.z * f.z) as f64).sqrt()
    }
}

struct JointDefBase {
    a: BodyId,
    b: BodyId,
    fa: Transform,
    fb: Transform,
}

impl JointDefBase {
    fn fill(&self, d: &mut JointDef) {
        d.body_id_a = self.a;
        d.body_id_b = self.b;
        d.local_frame_a = self.fa;
        d.local_frame_b = self.fb;
        // Collision proxies leave the joint regions out, so the connected parts may collide elsewhere.
        d.collide_connected = true;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Scenario {
    /// Held by its anchors, released as printed: does it swing down and hang straight?
    Hang,
    /// Held and shaken: does every joint move, does anything bind?
    Shake,
    /// Dropped on the bed: does it stay together, settle?
    Drop,
    /// Set to these joint angles (radians, per joint) and released: does it hold the pose?
    Pose(Vec<f64>),
}

impl Scenario {
    pub fn name(&self) -> &'static str {
        match self {
            Scenario::Hang => "hang",
            Scenario::Shake => "shake",
            Scenario::Drop => "drop",
            Scenario::Pose(_) => "pose",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimOptions {
    pub scenario: Scenario,
    /// Simulated seconds (budget: at most 30).
    pub seconds: f64,
    /// mm / s^2; none: the scenario's default.
    pub gravity: Option<P3>,
    pub hz: f64,
    pub seed: u64,
}

impl Default for SimOptions {
    fn default() -> SimOptions {
        SimOptions { scenario: Scenario::Hang, seconds: 8.0, gravity: None, hz: 240.0, seed: 1 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JointReport {
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub last: f64,
    /// The limits the joint had (lower, upper; cone: (0, cone)).
    pub limits: (f64, f64),
    /// Times it ran into a limit.
    pub limit_hits: u32,
    /// It turned by more than 3 degrees at some point.
    pub moved: bool,
    /// Predicted stick angle off hanging: asin(mu r / l), l = pivot to the moving side's centre of mass.
    pub dead_band: f64,
    /// At the end: the angle between gravity and the pivot-to-centre-of-mass line of the moving side (in the
    /// joint's free plane for a hinge). Hanging right: within the dead band.
    pub hang_error: f64,
    /// Peak constraint force, N.
    pub peak_force: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimReport {
    pub scenario: &'static str,
    pub seconds: f64,
    pub cancelled: bool,
    pub articulates: bool,
    /// Every joint ended within its dead band (+ 2 degrees) of hanging straight, or against a limit.
    pub hangs_right: bool,
    /// Joints that never moved.
    pub binding: Vec<String>,
    pub joints: Vec<JointReport>,
    /// Body pairs that touched (by name) and how often.
    pub self_contacts: Vec<(String, String, u32)>,
    pub floor_contacts: u32,
    /// Last time any part turned faster than 0.2 rad/s (none: never settled).
    pub settle: Option<f64>,
    /// Pose holding (pose scenario): largest joint drift, radians.
    pub drift: Option<f64>,
    /// Final pose per body: name, centre of mass, rotation (degrees about X, Y, Z).
    pub final_pose: Vec<(String, P3, P3)>,
    pub steps: u64,
    pub wall_ms: f64,
}

/// The joints' moving side: every body reached from `b` without crossing `j` (towards the leaves, away from
/// the anchors). Used for the dead-band lever.
fn moving_side(asm: &Assembly, j: usize) -> Vec<usize> {
    let anchored: Vec<usize> = asm.anchors.iter().map(|a| a.body).collect();
    let (a0, b0) = (asm.joints[j].a, asm.joints[j].b);
    let side = |start: usize, blocked: usize| -> Vec<usize> {
        let mut seen = vec![start];
        let mut stack = vec![start];
        while let Some(x) = stack.pop() {
            for (k, jj) in asm.joints.iter().enumerate() {
                if k == j {
                    continue;
                }
                let n = if jj.a == x { jj.b } else if jj.b == x { jj.a } else { continue };
                if n != blocked && !seen.contains(&n) {
                    seen.push(n);
                    stack.push(n);
                }
            }
        }
        seen
    };
    let sb = side(b0, a0);
    if sb.iter().any(|x| anchored.contains(x)) {
        side(a0, b0)
    } else {
        sb
    }
}

/// Set joints to angles by turning each joint's moving side about it (in joint order).
fn pose_bodies(asm: &Assembly, angles: &[f64]) -> Vec<(P3, [[f64; 3]; 3])> {
    let mut out: Vec<(P3, [[f64; 3]; 3])> = asm.bodies.iter().map(|b| (b.mass.com, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])).collect();
    for (j, ang) in angles.iter().enumerate().take(asm.joints.len()) {
        let js = &asm.joints[j];
        // the pivot and axis move with side A
        let mv = moving_side(asm, j);
        let axis = norm(js.axis);
        // a spherical joint tilts about the axis perpendicular to its cone axis and the print up
        let turn_axis = match js.kind {
            JointKind::Revolute { .. } => axis,
            JointKind::Spherical { .. } => {
                let t = cross(axis, norm(asm.up));
                if len(t) > 1e-6 {
                    norm(t)
                } else {
                    basis(axis).0
                }
            }
        };
        let sign = if mv.contains(&js.b) { 1.0 } else { -1.0 };
        for &i in &mv {
            let (c, r) = out[i];
            let nc = add(js.pivot, rotate(sub(c, js.pivot), turn_axis, ang * sign));
            let cols: [P3; 3] = std::array::from_fn(|k| rotate([r[0][k], r[1][k], r[2][k]], turn_axis, ang * sign));
            out[i] = (nc, std::array::from_fn(|a| std::array::from_fn(|b| cols[b][a])));
        }
    }
    out
}

fn mat_to_quat(m: &[[f64; 3]; 3]) -> Quat {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    Quat { v: vec3(x as f32, y as f32, z as f32), s: w as f32 }
}

/// A small deterministic generator for the shake.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub const MAX_BODIES: usize = 200;
pub const MAX_SECONDS: f64 = 30.0;

/// Run a scenario headless. `cancelled` is polled every 60 steps.
pub fn simulate(asm: &Assembly, opts: &SimOptions, cancelled: &dyn Fn() -> bool) -> Result<SimReport, String> {
    if asm.bodies.len() > MAX_BODIES {
        return Err(format!("{} parts: the sim budget is {MAX_BODIES}", asm.bodies.len()));
    }
    let started = std::time::Instant::now();
    let seconds = opts.seconds.clamp(0.1, MAX_SECONDS);
    let up = norm(asm.up);
    let gravity = opts.gravity.unwrap_or_else(|| match opts.scenario {
        Scenario::Drop => scale(up, -GRAVITY),
        _ => scale(asm.hang_gravity(), GRAVITY),
    });
    let mut held = asm.clone();
    if matches!(opts.scenario, Scenario::Drop) {
        held.anchors.clear();
    }
    let floor = if matches!(opts.scenario, Scenario::Drop) {
        let g = norm(gravity);
        let low = asm.bodies.iter().flat_map(|b| b.hulls.iter().flatten()).map(|p| dot(*p, g)).fold(f64::NEG_INFINITY, f64::max);
        Some(-(low + 20.0))
    } else {
        None
    };
    let mut w = MechWorld::new(&held, gravity, floor);
    if let Scenario::Pose(angles) = &opts.scenario {
        for (i, (c, r)) in pose_bodies(asm, angles).into_iter().enumerate() {
            body_set_transform(&mut w.world, w.bodies[i], to_pos(vf(c)), mat_to_quat(&r));
        }
    }
    let dt = 1.0 / opts.hz.max(60.0);
    let steps = (seconds / dt).round() as u64;
    let n = asm.joints.len();
    let start = w.angles(asm);
    let mut lo = start.clone();
    let mut hi = start.clone();
    let mut at_limit = vec![false; n];
    let mut hits = vec![0u32; n];
    let mut peak = vec![0.0f64; n];
    let mut contacts: std::collections::BTreeMap<(usize, usize), u32> = Default::default();
    let mut floor_contacts = 0;
    let mut settle = None;
    let mut rng = Rng(opts.seed.max(1).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let (shake_axis, shake_dir, f1, f2) = (up, norm(cross(up, norm(gravity))), 1.6 + rng.next(), 2.2 + rng.next());
    let mut cancelled_at = None;
    for s in 0..steps {
        if s % 60 == 0 && cancelled() {
            cancelled_at = Some(s);
            break;
        }
        let t = s as f64 * dt;
        if matches!(opts.scenario, Scenario::Shake) && t < seconds * 0.6 {
            let a = 18f64.to_radians() * (std::f64::consts::TAU * f1 * t).sin();
            let x = 12.0 * (std::f64::consts::TAU * f2 * t).sin();
            w.move_ground(scale(shake_dir, x), shake_axis, a, dt);
        } else if matches!(opts.scenario, Scenario::Shake) {
            w.move_ground([0.0; 3], shake_axis, 0.0, dt);
        }
        w.step(dt);
        for (a, b, _) in w.contact_begins() {
            match b {
                Some(b) if b != a => *contacts.entry((a, b)).or_default() += 1,
                None => floor_contacts += 1,
                _ => {}
            }
        }
        let ang = w.angles(asm);
        for j in 0..n {
            lo[j] = lo[j].min(ang[j]);
            hi[j] = hi[j].max(ang[j]);
            let (l, u) = limits_of(&asm.joints[j].kind);
            let near = ang[j] > u - 1f64.to_radians() || (matches!(asm.joints[j].kind, JointKind::Revolute { .. }) && ang[j] < l + 1f64.to_radians());
            if near && !at_limit[j] {
                hits[j] += 1;
            }
            at_limit[j] = near;
            peak[j] = peak[j].max(w.joint_force(j));
        }
        if w.max_angular_speed() > 0.2 {
            settle = Some((s + 1) as f64 * dt);
        }
    }
    let last = w.angles(asm);
    let poses = w.poses();
    let mut joints = Vec::new();
    for (j, js) in asm.joints.iter().enumerate() {
        let side = moving_side(asm, j);
        let (mut m, mut c) = (0.0, [0.0; 3]);
        for &i in &side {
            m += asm.bodies[i].mass.mass;
            c = madd(c, asm.bodies[i].mass.com, asm.bodies[i].mass.mass);
        }
        let lever = if m > 0.0 { len(sub(scale(c, 1.0 / m), js.pivot)) } else { 0.0 };
        let dead_band = if lever > 0.0 { (js.friction.mu * js.friction.radius / lever).min(1.0).asin() } else { 0.0 };
        // where the moving side's centre of mass is now, against gravity
        let (mut mm, mut cc) = (0.0, [0.0; 3]);
        for &i in &side {
            mm += asm.bodies[i].mass.mass;
            cc = madd(cc, poses[i].com, asm.bodies[i].mass.mass);
        }
        let fixed = if side.contains(&js.a) { js.b } else { js.a };
        let pivot_now = poses[fixed].apply(asm.bodies[fixed].mass.com, js.pivot);
        let axis_now = norm(sub(poses[fixed].apply(asm.bodies[fixed].mass.com, add(js.pivot, js.axis)), pivot_now));
        let gdir = norm(gravity);
        let hang_error = if mm > 0.0 {
            let r = sub(scale(cc, 1.0 / mm), pivot_now);
            let (r, g) = match js.kind {
                JointKind::Revolute { .. } => (sub(r, scale(axis_now, dot(r, axis_now))), sub(gdir, scale(axis_now, dot(gdir, axis_now)))),
                JointKind::Spherical { .. } => (r, gdir),
            };
            if len(r) < 1e-6 || len(g) < 1e-6 {
                0.0
            } else {
                dot(norm(r), norm(g)).clamp(-1.0, 1.0).acos()
            }
        } else {
            0.0
        };
        joints.push(JointReport {
            name: js.name.clone(),
            min: lo[j],
            max: hi[j],
            last: last[j],
            limits: limits_of(&js.kind),
            limit_hits: hits[j],
            moved: hi[j] - lo[j] > 3f64.to_radians(),
            dead_band,
            hang_error,
            peak_force: peak[j] * 1e-6,
        });
    }
    let binding: Vec<String> = joints.iter().filter(|j| !j.moved).map(|j| j.name.clone()).collect();
    let drift = match &opts.scenario {
        Scenario::Pose(_) => Some(joints.iter().zip(&start).map(|(j, s)| (j.last - s).abs()).fold(0.0, f64::max)),
        _ => None,
    };
    let final_pose = asm
        .bodies
        .iter()
        .zip(&poses)
        .map(|(b, p)| (b.name.clone(), p.com, quat_to_euler_deg(Quat { v: vec3(p.rot[0] as f32, p.rot[1] as f32, p.rot[2] as f32), s: p.rot[3] as f32 })))
        .collect();
    // Hanging from how it was printed need not move a joint that already hangs straight; a shake must move them all.
    let articulates = match opts.scenario {
        Scenario::Shake | Scenario::Drop => binding.is_empty(),
        _ => true,
    };
    let hangs_right = joints.iter().all(|j| j.hang_error <= j.dead_band + 2f64.to_radians() || j.limit_hits > 0 && (j.last - j.limits.0).abs().min((j.last - j.limits.1).abs()) < 2f64.to_radians());
    Ok(SimReport {
        hangs_right,
        scenario: opts.scenario.name(),
        seconds: cancelled_at.map(|s| s as f64 * dt).unwrap_or(seconds),
        cancelled: cancelled_at.is_some(),
        articulates,
        binding,
        joints,
        self_contacts: contacts.into_iter().map(|((a, b), c)| (asm.bodies[a].name.clone(), asm.bodies[b].name.clone(), c)).collect(),
        floor_contacts,
        settle,
        drift,
        final_pose,
        steps: cancelled_at.unwrap_or(steps),
        wall_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

fn limits_of(k: &JointKind) -> (f64, f64) {
    match k {
        JointKind::Revolute { lower, upper } => (*lower, *upper),
        JointKind::Spherical { cone, .. } => (0.0, *cone),
    }
}
