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
use makepad_box3d::shape::{create_hull_shape, create_tube_shape, shape_get_body};
use makepad_box3d::spherical_joint::*;
use makepad_box3d::types::*;

pub const GRAVITY: f64 = 9810.0;

/// Two surfaces against each other: Coulomb friction while they stick and while they slide, and the
/// coefficient of restitution of a hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub stick: f64,
    pub slide: f64,
    pub bounce: f64,
}

/// Printed PLA on printed PLA (layer lines, no lubricant).
pub const PLA_ON_PLA: Surface = Surface { stick: 0.35, slide: 0.28, bounce: 0.35 };
/// Printed PLA on a smooth build plate (smooth PEI or glass) and the other fixtures (the drape ball).
pub const PLA_ON_PLATE: Surface = Surface { stick: 0.30, slide: 0.22, bounce: 0.25 };
/// A body below this speed (mm/s, its centre plus its spin at its farthest point) sticks: its contacts
/// use the sticking friction.
pub const STICK_SPEED: f64 = 5.0;
/// Hits slower than this (mm/s) do not bounce, so resting parts and stacks stay quiet.
pub const BOUNCE_SPEED: f64 = 150.0;
/// Air drag on a free part (1/s): a light linear and angular damping, so a throw still flies.
pub const AIR_LINEAR_DAMPING: f64 = 0.05;
pub const AIR_ANGULAR_DAMPING: f64 = 0.1;
/// Contacts are made this far apart (mm), ahead of the touch: a part falling at 1 m/s moves 4 mm per
/// 240 Hz step, and a contact made only once it is inside the other sinks it that deep before it is pushed
/// out (and that push, not the hit, decides the bounce). box3d's own default is metre-tuned (20 um here).
pub const CONTACT_MARGIN: f64 = 3.0;
/// Contact stiffness (Hz): a part sinks into what it rests on by about g / (2 pi f)^2 per weight it
/// carries; box3d's default 30 Hz sinks a part a quarter millimetre, as much as a print-in-place
/// clearance. 240 Hz (a quarter of 240 Hz times 4 substeps, what a weave's hanging cloth runs at) sinks
/// a stack of parts a few hundredths.
pub const CONTACT_HERTZ: f64 = 240.0;
/// Parts slower than this (mm/s) for half a second fall asleep and stop completely.
pub const SLEEP_SPEED: f64 = 2.0;
/// A grab's spring follows the cursor critically damped: no overshoot.
pub const GRAB_DAMPING_RATIO: f64 = 1.0;
/// A held part swings about the grip, damped to this ratio by the grip (the fingers' viscous hold).
pub const GRIP_SWING_DAMPING_RATIO: f64 = 0.7;
/// The grip's least angular damping (1/s): a part held at its centre (no swing) still stops spinning.
pub const GRIP_SPIN_DAMPING: f64 = 6.0;
/// The grip's dry friction against turning, as a fraction of the part's weight times the grip's lever: a
/// swing ends instead of creeping on forever.
pub const GRIP_STICTION: f64 = 0.03;
/// The hand's velocity (the grip damps against it) is smoothed over a display frame (s).
pub const HAND_SMOOTHING: f64 = 1.0 / 60.0;
/// A dangle's soft spring (Hz): the part sags and bobs on it, critically damped.
pub const DANGLE_HZ: f64 = 2.5;

/// Surface classes, carried in a shape's material id; the flag marks a body that is sliding.
const MAT_PLA: u64 = 1;
const MAT_PLATE: u64 = 2;
const MAT_SLIDING: u64 = 1 << 8;

fn surface(a: u64, b: u64) -> Surface {
    if (a | b) & 0xff & MAT_PLATE != 0 {
        PLA_ON_PLATE
    } else {
        PLA_ON_PLA
    }
}

/// The world's friction: the pair's sticking friction while both stick, else its sliding friction.
fn pair_friction(_fa: f32, a: u64, _fb: f32, b: u64) -> f32 {
    let s = surface(a, b);
    (if (a | b) & MAT_SLIDING != 0 { s.slide } else { s.stick }) as f32
}

fn pair_restitution(_ra: f32, a: u64, _rb: f32, b: u64) -> f32 {
    surface(a, b).bounce as f32
}

fn material(class: u64) -> makepad_box3d::types::SurfaceMaterial {
    let s = surface(class, class);
    makepad_box3d::types::SurfaceMaterial { friction: s.stick as f32, restitution: s.bounce as f32, user_material_id: class, ..Default::default() }
}

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
    /// Closed tubes (model frame: centreline loop, radius) for collision: the exact solid of a link.
    pub tubes: Vec<(Vec<P3>, f64)>,
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
    /// Bodies resting below this speed (mm/s) fall asleep (0: never; [`SLEEP_SPEED`] for printed parts).
    pub sleep: f64,
    /// Solver threads (0 or 1: one, the deterministic default for joints; results match across counts).
    pub workers: u32,
    /// Contacts are made this close (mm; 0: [`CONTACT_MARGIN`]). Thin fast links need about their bar
    /// diameter, or they pass through each other between steps.
    pub contact_margin: f64,
    /// Speed cap, mm/s (0: none).
    pub max_speed: f64,
    /// Contact stiffness, Hz (0: [`CONTACT_HERTZ`]; capped at 1/4 of the substep rate). Stacked loads (a
    /// cloth hanging from its top row) sink into soft contacts by about load / stiffness.
    pub contact_hertz: f64,
    /// Substeps per step (0: 4).
    pub substeps: u32,
    /// A fixed ball (centre, radius) to drape over.
    pub ball: Option<(P3, f64)>,
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

    /// A model-frame direction seen from the body as built (its rotation undone).
    pub fn unrotate(&self, d: P3) -> P3 {
        let q = Quat { v: vec3(self.rot[0] as f32, self.rot[1] as f32, self.rot[2] as f32), s: self.rot[3] as f32 };
        pf(makepad_box3d::math_functions::inv_rotate_vector(q, vf(d)))
    }
}

/// The ground: a slab far wider than any scene and thick enough that a fast part cannot cross
/// it in one step (mm, half extents; its top face is the floor level).
const GROUND_HALF_SIZE: f32 = 200_000.0;
const GROUND_HALF_THICKNESS: f64 = 1000.0;

/// A convex collision hull of `pts` that always exists: a dense cloud (a meshed distance field)
/// can overrun box3d's hull limits, so it is built with fewer vertices until it fits, and in the
/// end from its bounding box. A body is never left without a shape.
fn collision_hull(pts: &[Vec3]) -> std::sync::Arc<makepad_box3d::types::HullData> {
    for max in [64, 32, 16, 8] {
        if let Some(h) = create_hull(pts, max) {
            return h;
        }
    }
    let (mut lo, mut hi) = (Vec3 { x: f32::MAX, y: f32::MAX, z: f32::MAX }, Vec3 { x: f32::MIN, y: f32::MIN, z: f32::MIN });
    for p in pts {
        lo = Vec3 { x: lo.x.min(p.x), y: lo.y.min(p.y), z: lo.z.min(p.z) };
        hi = Vec3 { x: hi.x.max(p.x), y: hi.y.max(p.y), z: hi.z.max(p.z) };
    }
    // a flat cloud's box is given a little thickness, so it is a solid
    let pad = 0.01 * (hi.x - lo.x).max(hi.y - lo.y).max(hi.z - lo.z).max(1.0);
    let grow = |l: f32, h: f32| if h - l < pad { (0.5 * (l + h) - 0.5 * pad, 0.5 * (l + h) + 0.5 * pad) } else { (l, h) };
    let ((lx, hx), (ly, hy), (lz, hz)) = (grow(lo.x, hi.x), grow(lo.y, hi.y), grow(lo.z, hi.z));
    let corners: Vec<Vec3> = (0..8).map(|i| Vec3 { x: if i & 1 == 0 { lx } else { hx }, y: if i & 2 == 0 { ly } else { hy }, z: if i & 4 == 0 { lz } else { hz } }).collect();
    create_hull(&corners, 8).expect("a box of eight corners is a hull")
}

struct Grab {
    target: BodyId,
    joint: JointId,
    body: usize,
    local: Vec3,
    /// Where the spring's end is now (it follows the cursor no faster than the world's speed cap), and
    /// how fast it moves.
    at: P3,
    velocity: P3,
    /// The grip's damping of the part's motion against the hand's (1/s).
    linear_damping: f64,
}

struct Pin {
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
    pins: Vec<Pin>,
    /// Body-to-body joints only: contacts and kinematic targets do not join throw islands.
    neighbours: Vec<Vec<usize>>,
    floor: Option<BodyId>,
    /// The assembly's anchors as built: the held body and its joint (a weld or pin to the ground).
    anchors: Vec<(usize, JointId)>,
    /// Bodies stopped in place by [`MechWorld::freeze`], with the damping they had.
    frozen: Vec<(usize, (f32, f32))>,
    pub gravity: P3,
    time: f64,
    ground_pose: (P3, f64),
    substeps: i32,
    /// Per body: its shapes, its reach from its centre of mass (mm), and whether it is sliding.
    shapes: Vec<Vec<makepad_box3d::id::ShapeId>>,
    reach: Vec<f64>,
    sliding: Vec<bool>,
    /// The assembly lets resting bodies sleep.
    sleeps: bool,
    /// The assembly's speed cap, mm/s (0: none).
    max_speed: f64,
}

impl MechWorld {
    pub fn new(asm: &Assembly, gravity: P3, floor: Option<f64>) -> MechWorld {
        let mut def = default_world_def();
        def.gravity = vf(gravity);
        def.maximum_linear_speed = if asm.max_speed > 0.0 { asm.max_speed as f32 } else { 400_000.0 };
        def.speculative_distance = if asm.contact_margin > 0.0 { asm.contact_margin } else { CONTACT_MARGIN } as f32;
        // the stiffness holds up to a quarter of the substep rate
        def.contact_hertz = if asm.contact_hertz > 0.0 { asm.contact_hertz } else { CONTACT_HERTZ } as f32;
        def.contact_hertz_ratio = 0.25;
        def.contact_speed = 3000.0;
        def.restitution_threshold = BOUNCE_SPEED as f32;
        def.friction_callback = Some(pair_friction);
        def.restitution_callback = Some(pair_restitution);
        def.worker_count = asm.workers.max(1);
        let mut world = create_world(&def);
        let mut gd = default_body_def();
        gd.body_type = BodyType::Kinematic;
        let ground = create_body(&mut world, &gd);
        let mut bodies = Vec::new();
        let mut rest = Vec::new();
        let mut shapes = Vec::new();
        let mut reach = Vec::new();
        for (i, b) in asm.bodies.iter().enumerate() {
            let mut bd = default_body_def();
            bd.body_type = BodyType::Dynamic;
            bd.position = to_pos(vf(b.mass.com));
            bd.enable_sleep = asm.sleep > 0.0;
            bd.sleep_threshold = asm.sleep as f32;
            bd.linear_damping = AIR_LINEAR_DAMPING as f32;
            bd.angular_damping = AIR_ANGULAR_DAMPING as f32;
            let id = create_body(&mut world, &bd);
            let mut sd = default_shape_def();
            sd.base_material = material(MAT_PLA);
            sd.enable_contact_events = true;
            sd.enable_hit_events = true;
            sd.update_body_mass = false;
            sd.user_data = i as u64;
            let mut own = Vec::new();
            for h in &b.hulls {
                let pts: Vec<Vec3> = h.iter().map(|p| vf(sub(*p, b.mass.com))).collect();
                own.push(create_hull_shape(&mut world, id, &sd, &collision_hull(&pts)));
            }
            for (pts, r) in &b.tubes {
                let tube = Tube { points: pts.iter().map(|p| vf(sub(*p, b.mass.com))).collect(), radius: *r as f32 };
                own.push(create_tube_shape(&mut world, id, &sd, &tube));
            }
            shapes.push(own);
            let far = b.hulls.iter().flatten().map(|p| len(sub(*p, b.mass.com))).fold(0.0, f64::max);
            reach.push(b.tubes.iter().flat_map(|(pts, r)| pts.iter().map(move |p| len(sub(*p, b.mass.com)) + r)).fold(far, f64::max));
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
        let mut anchors = Vec::new();
        for a in &asm.anchors {
            match &a.kind {
                AnchorKind::Fixed => {
                    let mut d = default_weld_joint_def();
                    d.base.body_id_a = ground;
                    d.base.body_id_b = bodies[a.body];
                    d.base.local_frame_a = Transform { p: vf(a.pivot), q: Quat::IDENTITY };
                    d.base.local_frame_b = Transform { p: local(&rest, a.body, a.pivot), q: Quat::IDENTITY };
                    anchors.push((a.body, makepad_box3d::joint::create_weld_joint(&mut world, &d)));
                }
                AnchorKind::Pin(axis) => {
                    let q = quat_z_to(*axis);
                    let mut d = default_revolute_joint_def();
                    d.base.body_id_a = ground;
                    d.base.body_id_b = bodies[a.body];
                    d.base.local_frame_a = Transform { p: vf(a.pivot), q };
                    d.base.local_frame_b = Transform { p: local(&rest, a.body, a.pivot), q };
                    d.base.friction = JointFriction { coefficient: a.friction.mu as f32, radius: a.friction.radius as f32, torque: a.friction.torque as f32 };
                    anchors.push((a.body, create_revolute_joint(&mut world, &d)));
                }
            }
        }
        let floor_id = floor.map(|level| {
            let g = norm(gravity);
            let mut bd = default_body_def();
            bd.position = to_pos(vf(scale(g, -level + GROUND_HALF_THICKNESS)));
            bd.rotation = quat_z_to(scale(g, -1.0));
            let id = create_body(&mut world, &bd);
            let mut sd = default_shape_def();
            sd.base_material = material(MAT_PLATE);
            sd.user_data = u64::MAX;
            create_hull_shape(&mut world, id, &sd, &makepad_box3d::hull::make_box_hull(GROUND_HALF_SIZE, GROUND_HALF_SIZE, GROUND_HALF_THICKNESS as f32));
            id
        });
        let mut neighbours = vec![Vec::new(); bodies.len()];
        for j in &asm.joints {
            neighbours[j.a].push(j.b);
            neighbours[j.b].push(j.a);
        }
        let sliding = vec![false; bodies.len()];
        let mut mw = MechWorld { world, ground, bodies, joints, rest, grab: None, pins: Vec::new(), neighbours, floor: floor_id, anchors, frozen: Vec::new(), gravity, time: 0.0, ground_pose: ([0.0; 3], 0.0), substeps: if asm.substeps > 0 { asm.substeps as i32 } else { 4 }, shapes, reach, sliding, sleeps: asm.sleep > 0.0, max_speed: asm.max_speed };
        if let Some((c, r)) = asm.ball {
            mw.add_ball(c, r);
        }
        mw
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    /// One fixed step (`dt` seconds, always `substeps` solver substeps).
    pub fn step(&mut self, dt: f64) {
        self.update_sliding();
        self.damp_grip(dt);
        world_step(&mut self.world, dt as f32, self.substeps);
        self.time += dt;
    }

    /// The grip damps the held part's velocity against the hand's, implicitly (stable at any rate).
    fn damp_grip(&mut self, dt: f64) {
        let Some(g) = &self.grab else { return };
        let b = self.bodies[g.body];
        let v = pf(body_get_linear_velocity(&self.world, b));
        let keep = 1.0 / (1.0 + dt * g.linear_damping);
        let damped = add(g.velocity, scale(sub(v, g.velocity), keep));
        body_set_linear_velocity(&mut self.world, b, vf(damped));
    }

    /// Each awake body's contacts stick or slide by its speed (a sleeping body sticks). Only a change
    /// touches its shapes.
    fn update_sliding(&mut self) {
        for i in 0..self.bodies.len() {
            let b = self.bodies[i];
            let slide = body_is_awake(&self.world, b) && {
                let v = body_get_linear_velocity(&self.world, b);
                let w = body_get_angular_velocity(&self.world, b);
                let (v, w) = (((v.x * v.x + v.y * v.y + v.z * v.z) as f64).sqrt(), ((w.x * w.x + w.y * w.y + w.z * w.z) as f64).sqrt());
                v + w * self.reach[i] > STICK_SPEED
            };
            if slide != self.sliding[i] {
                self.sliding[i] = slide;
                let mut m = material(MAT_PLA);
                if slide {
                    m.user_material_id |= MAT_SLIDING;
                }
                for s in &self.shapes[i] {
                    makepad_box3d::shape::shape_set_surface_material(&mut self.world, *s, m);
                }
            }
        }
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
        // the ground's joints do not join islands: what hangs from it is woken to follow it
        for i in 0..self.anchors.len() {
            let b = self.bodies[self.anchors[i].0];
            if !body_is_awake(&self.world, b) {
                body_set_awake(&mut self.world, b, true);
            }
        }
    }

    /// Start dragging `body` by the model point `point` (where it is now): a critically damped spring of
    /// `stiffness_hz` pulls the point to the cursor, and the grip damps the swing about it.
    pub fn grab(&mut self, body: usize, point: P3, stiffness_hz: f64) {
        self.spring(body, point, stiffness_hz);
    }

    /// Hang from a soft spring; moved and released like a firm grab.
    pub fn dangle(&mut self, body: usize, point: P3) {
        self.spring(body, point, DANGLE_HZ);
    }

    fn spring(&mut self, body: usize, point: P3, stiffness_hz: f64) {
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
        d.linear_damping_ratio = GRAB_DAMPING_RATIO as f32;
        // 200 times the part's weight, and at least ten times everything's: a part held up by a
        // corner of a sheet of hundreds of links lifts the sheet
        let all: f64 = self.bodies.iter().map(|id| body_get_mass(&self.world, *id) as f64).sum();
        d.max_spring_force = ((mass * 200.0).max(all * 10.0) * GRAVITY) as f32;
        d.angular_hertz = 0.0;
        d.max_spring_torque = 0.0;
        d.max_velocity_force = 0.0;
        // The grip: the swing about the held point as a pendulum (lever r, inertia about the centre across
        // the lever i, about the grip i + m r^2) damped to GRIP_SWING_DAMPING_RATIO, a damping of
        // 2 zeta sqrt((i + m r^2) m g r); and a dry torque that ends the last of it.
        let lever = sub(point, pf(to_vec3(body_get_world_center_of_mass(&self.world, b))));
        let r = len(lever);
        let i_across = self.inertia_across(body, lever);
        let g = len(self.gravity);
        // The swing's damping (torque per rad/s about the grip) comes from the part's motion against the
        // hand (m r^2 c_linear) and its turn (i c_angular); the linear share also damps the part bobbing on a
        // soft spring: its whole mass on a spring tuned for the held point's effective mass
        // (1/m_eff = 1/m + r^2/i), critically damped at most.
        let swing = 2.0 * GRIP_SWING_DAMPING_RATIO * ((i_across + mass * r * r) * mass * g * r).sqrt();
        let m_eff = 1.0 / (1.0 / mass + r * r / i_across.max(1e-9));
        let bob = 2.0 * GRAB_DAMPING_RATIO * std::f64::consts::TAU * stiffness_hz * (m_eff / mass).sqrt();
        let linear_damping = if r > 1e-6 { bob.min(swing / (mass * r * r)) } else { bob };
        let angular_damping = ((swing - mass * r * r * linear_damping) / i_across.max(1e-9)).max(GRIP_SPIN_DAMPING);
        d.max_velocity_torque = (GRIP_STICTION * mass * g * r) as f32;
        let joint = create_motor_joint(&mut self.world, &d);
        body_set_angular_damping(&mut self.world, b, angular_damping as f32);
        // held, it follows the cursor: it must not fall asleep under it
        body_enable_sleep(&mut self.world, b, false);
        body_set_awake(&mut self.world, b, true);
        self.grab = Some(Grab { target, joint, body, local: local_b, at: point, velocity: [0.0; 3], linear_damping });
    }

    /// Body `i`'s moment of inertia (g mm^2, about its centre of mass) across the world direction `d`: the
    /// mean over the axes at right angles to it.
    fn inertia_across(&self, i: usize, d: P3) -> f64 {
        let b = self.bodies[i];
        let t = body_get_transform(&self.world, b);
        let local = if len(d) > 1e-9 { norm(pf(makepad_box3d::math_functions::inv_rotate_vector(t.q, vf(d)))) } else { [0.0, 0.0, 1.0] };
        let m = body_get_mass_data(&self.world, b).inertia;
        let col = [m.cx, m.cy, m.cz];
        let im = |a: P3| -> P3 { std::array::from_fn(|r| (0..3).map(|c| [col[c].x, col[c].y, col[c].z][r] as f64 * a[c]).sum()) };
        let trace = (col[0].x + col[1].y + col[2].z) as f64;
        0.5 * (trace - dot(local, im(local)))
    }

    /// Toggle a point anchor at its current world position. It allows rotation and stays put
    /// while the assembly's original anchors shake. Rebuilding the world clears these pins.
    pub fn pin(&mut self, body: usize, point: P3) {
        if let Some(i) = self.pins.iter().position(|p| p.body == body) {
            let pin = self.pins.remove(i);
            destroy_joint(&mut self.world, pin.joint, true);
            destroy_body(&mut self.world, pin.target);
            return;
        }
        let mut bd = default_body_def();
        bd.body_type = BodyType::Kinematic;
        bd.position = to_pos(vf(point));
        let target = create_body(&mut self.world, &bd);
        let b = self.bodies[body];
        let mut d = default_spherical_joint_def();
        d.base.body_id_a = target;
        d.base.body_id_b = b;
        d.base.local_frame_a = Transform::IDENTITY;
        d.base.local_frame_b = Transform { p: body_get_local_point(&self.world, b, to_pos(vf(point))), q: Quat::IDENTITY };
        let joint = create_spherical_joint(&mut self.world, &d);
        self.pins.push(Pin { target, joint, body });
    }

    pub fn pinned(&self) -> Vec<usize> {
        self.pins.iter().map(|p| p.body).collect()
    }

    /// Apply a mass-scaled impulse at a world point: `dv` changes the centre's linear velocity
    /// (model mm/s), and an off-centre hit also spins the body.
    pub fn poke(&mut self, body: usize, point: P3, dv: P3) {
        let b = self.bodies[body];
        let impulse = vf(scale(dv, body_get_mass(&self.world, b) as f64));
        body_apply_linear_impulse(&mut self.world, b, impulse, to_pos(vf(point)), true);
    }

    /// Move the grab target (model point); the part follows through the spring. The spring's end chases
    /// the cursor at most at the world's speed cap: a flick faster than the parts may move (a weave's
    /// links, capped so they cannot pass through each other) does not stretch the spring into a yank that
    /// drives links through each other.
    pub fn drag(&mut self, point: P3, dt: f64) {
        let cap = self.max_speed * dt;
        if let Some(g) = &mut self.grab {
            let d = sub(point, g.at);
            let l = len(d);
            let at = if cap > 0.0 && l > cap { madd(g.at, d, cap / l) } else { point };
            if dt > 0.0 {
                // the cursor moves once per display frame while the sim steps several times: its velocity
                // is smoothed over a frame, not a jump in one step and rest in the next
                let raw = scale(sub(at, g.at), 1.0 / dt);
                g.velocity = add(g.velocity, scale(sub(raw, g.velocity), dt / (dt + HAND_SMOOTHING)));
            }
            g.at = at;
            body_set_target_transform(&mut self.world, g.target, WorldTransform { p: to_pos(vf(g.at)), q: Quat::IDENTITY }, dt as f32, true);
        }
    }

    /// The held point leaves at `velocity` (model frame, mm/s). Shift the whole joint island by
    /// the same velocity change, keeping its relative motion and every body's spin. Contacts
    /// alone do not carry a throw. Call before [`MechWorld::release`].
    pub fn throw(&mut self, velocity: P3) {
        let Some(g) = &self.grab else { return };
        let b = self.bodies[g.body];
        // Use the body's actual held point, not the spring target (which can lead it).
        let at = body_get_world_point(&self.world, b, g.local);
        let now = body_get_world_point_velocity(&self.world, b, at);
        let want = vf(velocity);
        let dv = Vec3 { x: want.x - now.x, y: want.y - now.y, z: want.z - now.z };
        let mut seen = vec![false; self.bodies.len()];
        let mut pending = vec![g.body];
        seen[g.body] = true;
        while let Some(i) = pending.pop() {
            let b = self.bodies[i];
            let v = body_get_linear_velocity(&self.world, b);
            body_set_linear_velocity(&mut self.world, b, Vec3 { x: v.x + dv.x, y: v.y + dv.y, z: v.z + dv.z });
            for &next in &self.neighbours[i] {
                if !seen[next] {
                    seen[next] = true;
                    pending.push(next);
                }
            }
        }
    }

    /// Let go (the part keeps its velocity: a throw).
    pub fn release(&mut self) {
        if let Some(g) = self.grab.take() {
            let b = self.bodies[g.body];
            body_set_linear_damping(&mut self.world, b, AIR_LINEAR_DAMPING as f32);
            body_set_angular_damping(&mut self.world, b, AIR_ANGULAR_DAMPING as f32);
            body_enable_sleep(&mut self.world, b, self.sleeps);
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

    /// The fastest part, mm/s.
    pub fn max_speed(&self) -> f64 {
        self.bodies
            .iter()
            .map(|b| {
                let v = body_get_linear_velocity(&self.world, *b);
                ((v.x * v.x + v.y * v.y + v.z * v.z) as f64).sqrt()
            })
            .fold(0.0, f64::max)
    }

    /// A fixed ball (a 40-point hull: box3d hulls hold at most 255 half edges; contacts are made early like the links') to drape things over.
    pub fn add_ball(&mut self, centre: P3, radius: f64) {
        let mut bd = default_body_def();
        bd.position = to_pos(vf(centre));
        let id = create_body(&mut self.world, &bd);
        let mut pts = Vec::new();
        // a Fibonacci sphere
        let n = 40;
        for i in 0..n {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let rr = (1.0 - y * y).sqrt();
            let a = i as f64 * 2.399963229728653;
            pts.push(vec3((radius * rr * a.cos()) as f32, (radius * y) as f32, (radius * rr * a.sin()) as f32));
        }
        let mut sd = default_shape_def();
        sd.base_material = material(MAT_PLATE);
        sd.user_data = u64::MAX;
        if let Some(hull) = create_hull(&pts, 64) {
            create_hull_shape(&mut self.world, id, &sd, &hull);
        }
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

/// What the Mechanics view does to chosen bodies ([`MechWorld::apply`]).
#[derive(Clone, Debug, PartialEq)]
pub enum BodyAction {
    /// Let go (pins, anchors, a hold) and, when the body was held, leave it be; a free one is lifted
    /// `lift` mm against gravity first, so it visibly falls back.
    Drop { lift: f64 },
    /// Let go, then hang the set from a pin at the top of its highest body (against gravity).
    Hang,
    /// Pin each body where its centre is now.
    Pin,
    Unpin,
    /// Change the velocity by `dv` (model frame, mm/s).
    Throw(P3),
    /// Back to the print pose, at rest, free.
    Home,
    /// Stop in place (no gravity, no motion) or move again.
    Freeze(bool),
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

/// Bodies per solver thread. A step of a few hundred bodies is a millisecond or two on one thread; split
/// over threads it waits at every colour and substep for the slowest of them, and one thread the OS puts
/// aside (a busy machine) stalls the step for tens of milliseconds.
pub const BODIES_PER_WORKER: usize = 400;

/// Solver threads for a world of `bodies`: one until the bodies fill more, at most the cores but two.
pub fn solver_workers(bodies: usize) -> u32 {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).saturating_sub(2).max(1);
    bodies.div_ceil(BODIES_PER_WORKER).clamp(1, cores.min(8)) as u32
}
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

impl MechWorld {
    /// Drop the pins, anchors and hold of `body`; true when it was held by any.
    fn let_go(&mut self, body: usize) -> bool {
        let mut held = false;
        if self.grab.as_ref().is_some_and(|g| g.body == body) {
            self.release();
            held = true;
        }
        while let Some(i) = self.pins.iter().position(|p| p.body == body) {
            let pin = self.pins.remove(i);
            destroy_joint(&mut self.world, pin.joint, true);
            destroy_body(&mut self.world, pin.target);
            held = true;
        }
        while let Some(i) = self.anchors.iter().position(|a| a.0 == body) {
            let (_, joint) = self.anchors.remove(i);
            destroy_joint(&mut self.world, joint, true);
            held = true;
        }
        body_set_awake(&mut self.world, self.bodies[body], true);
        held
    }

    /// The body's highest point along `up` (its box's corner), model frame.
    fn top(&self, body: usize, up: P3) -> f64 {
        let a = body_compute_aabb(&self.world, self.bodies[body]);
        let (lo, hi) = (a.lower_bound, a.upper_bound);
        let mut best = f64::NEG_INFINITY;
        for i in 0..8 {
            let c = [if i & 1 == 0 { lo.x } else { hi.x } as f64, if i & 2 == 0 { lo.y } else { hi.y } as f64, if i & 4 == 0 { lo.z } else { hi.z } as f64];
            best = best.max(dot(c, up));
        }
        best
    }

    fn stand_still(&mut self, body: usize) {
        let b = self.bodies[body];
        body_set_linear_velocity(&mut self.world, b, Vec3::ZERO);
        body_set_angular_velocity(&mut self.world, b, Vec3::ZERO);
    }

    pub fn is_frozen(&self, body: usize) -> bool {
        self.frozen.iter().any(|f| f.0 == body)
    }

    pub fn frozen(&self) -> Vec<usize> {
        self.frozen.iter().map(|f| f.0).collect()
    }

    /// Do `action` to each of `bodies`.
    pub fn apply(&mut self, bodies: &[usize], action: &BodyAction) {
        let bodies: Vec<usize> = bodies.iter().copied().filter(|b| *b < self.bodies.len()).collect();
        if bodies.is_empty() {
            return;
        }
        match action {
            BodyAction::Drop { lift } => {
                for &b in &bodies {
                    self.freeze_one(b, false);
                    let held = self.let_go(b);
                    if !held && *lift > 0.0 {
                        let g = norm(self.gravity);
                        let id = self.bodies[b];
                        let t = body_get_transform(&self.world, id);
                        let p = to_vec3(t.p);
                        let to = vec3(p.x - (g[0] * lift) as f32, p.y - (g[1] * lift) as f32, p.z - (g[2] * lift) as f32);
                        body_set_transform(&mut self.world, id, to_pos(to), t.q);
                        self.stand_still(b);
                    }
                }
            }
            BodyAction::Hang => {
                let up = scale(norm(self.gravity), -1.0);
                for &b in &bodies {
                    self.freeze_one(b, false);
                    self.let_go(b);
                }
                // Lift the set by its own size, so what dangles from the pin clears the floor.
                let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
                for &b in &bodies {
                    let a = body_compute_aabb(&self.world, self.bodies[b]);
                    for k in 0..3 {
                        lo[k] = lo[k].min([a.lower_bound.x, a.lower_bound.y, a.lower_bound.z][k] as f64);
                        hi[k] = hi[k].max([a.upper_bound.x, a.upper_bound.y, a.upper_bound.z][k] as f64);
                    }
                }
                let size = len(sub(hi, lo));
                for &b in &bodies {
                    let id = self.bodies[b];
                    let t = body_get_transform(&self.world, id);
                    let p = to_vec3(t.p);
                    let to = vec3(p.x + (up[0] * size) as f32, p.y + (up[1] * size) as f32, p.z + (up[2] * size) as f32);
                    body_set_transform(&mut self.world, id, to_pos(to), t.q);
                    self.stand_still(b);
                }
                let Some(&top) = bodies.iter().max_by(|a, b| self.top(**a, up).total_cmp(&self.top(**b, up))) else { return };
                let id = self.bodies[top];
                let com = pf(to_vec3(body_get_world_center_of_mass(&self.world, id)));
                let a = body_compute_aabb(&self.world, id);
                let half = 0.5 * len(sub([a.upper_bound.x as f64, a.upper_bound.y as f64, a.upper_bound.z as f64], [a.lower_bound.x as f64, a.lower_bound.y as f64, a.lower_bound.z as f64]));
                // off the centre line, so it swings to hang
                let at = madd(madd(com, up, (self.top(top, up) - dot(com, up)).max(0.0)), basis(up).0, 0.25 * half);
                self.pin(top, at);
            }
            BodyAction::Pin => {
                for &b in &bodies {
                    if !self.pins.iter().any(|p| p.body == b) {
                        let com = pf(to_vec3(body_get_world_center_of_mass(&self.world, self.bodies[b])));
                        self.pin(b, com);
                    }
                }
            }
            BodyAction::Unpin => {
                for &b in &bodies {
                    self.let_go(b);
                }
            }
            BodyAction::Throw(dv) => {
                for &b in &bodies {
                    self.freeze_one(b, false);
                    self.let_go(b);
                    let id = self.bodies[b];
                    let v = body_get_linear_velocity(&self.world, id);
                    body_set_linear_velocity(&mut self.world, id, vec3(v.x + dv[0] as f32, v.y + dv[1] as f32, v.z + dv[2] as f32));
                }
            }
            BodyAction::Home => {
                for &b in &bodies {
                    self.freeze_one(b, false);
                    self.let_go(b);
                    body_set_transform(&mut self.world, self.bodies[b], to_pos(vf(self.rest[b])), Quat::IDENTITY);
                    self.stand_still(b);
                }
            }
            BodyAction::Freeze(on) => {
                for &b in &bodies {
                    self.freeze_one(b, *on);
                }
            }
        }
    }

    fn freeze_one(&mut self, body: usize, on: bool) {
        let id = self.bodies[body];
        if on && !self.is_frozen(body) {
            self.frozen.push((body, (body_get_linear_damping(&self.world, id), body_get_angular_damping(&self.world, id))));
            self.stand_still(body);
            body_set_gravity_scale(&mut self.world, id, 0.0);
            body_set_linear_damping(&mut self.world, id, 1000.0);
            body_set_angular_damping(&mut self.world, id, 1000.0);
        } else if !on {
            if let Some(i) = self.frozen.iter().position(|f| f.0 == body) {
                let (_, (lin, ang)) = self.frozen.remove(i);
                body_set_gravity_scale(&mut self.world, id, 1.0);
                body_set_linear_damping(&mut self.world, id, lin);
                body_set_angular_damping(&mut self.world, id, ang);
                body_set_awake(&mut self.world, id, true);
            }
        }
    }
}
