//! Mechanisms in millimetres, grams and seconds on the dynamics engine.
//!
//! One addition of ours to the engine's method, kept here so the engine stays as translated: loop
//! closures are projected back after each substep (`close_loops`). The engine holds an equality only at
//! the acceleration level, so a loop closed by a weld drifts open by about v^2 tau^2, its velocity squared
//! times the weld's time constant squared (0.024 mm on a spatial loop turning at 30 rad/s, 1.9e-3 mm at a
//! four-bar's blow on its base). Two Newton steps on positions and one on velocities, in the mass metric,
//! hold a closure as a tree joint holds (1e-14 mm), and take no measurable energy: a hanging four-bar keeps
//! 99.97% of its swing over 3 s (tests/dyn_world.rs checks 99%).
//!
//! And one rule of ours on the engine's island sleep: a tree may sleep only once it has been under the
//! assembly's sleep speed for half a second (`gate_sleep`, the current engine's rule). The engine alone
//! waits ten substeps, after which a slow pendulum would stop at the top of its swing.

use crate::geom::*;
use crate::sim::{AnchorKind, Assembly, BodyAction, Friction, JointKind, Motor, Pose, StringFault, StringPath, StringSpec, NEWTON, AIR_ANGULAR_DAMPING,
    AIR_LINEAR_DAMPING, BOUNCE_SPEED, CONTACT_HERTZ, DANGLE_HZ, GRAB_DAMPING_RATIO, GRIP_SPIN_DAMPING, GRIP_STICTION,
    GRIP_SWING_DAMPING_RATIO, GRAVITY, HAND_SMOOTHING, PLA_ON_PLA};
use makepad_dynamics::{builder, collision_driver, core_smooth, core_util, forward, io, name, sleep, support, util_solve, util_spatial};
use makepad_dynamics::data::Data;
use makepad_dynamics::model::Model;
use makepad_dynamics::model_spec::{ActuatorSpec, BodySpec, EqualitySpec, ExcludeSpec, GeomSpec, JointSpec, MeshSpec, ModelSpec, PairSpec, SiteSpec, TendonSpec, WrapSpec};
use makepad_dynamics::types::*;
use makepad_dynamics::sdf::{self, Sdf};
use makepad_box3d::field::DistanceField;
use std::collections::{BTreeMap, BTreeSet};
use makepad_dynamics::thread::TaskPool;
use std::sync::Arc;

// The sliding coefficient is to be chosen by measurement of the printed surfaces.
const CONTACT_MU: f64 = PLA_ON_PLA.slide;
// A tree stays under the assembly's sleep speed this long before the engine may put it to sleep (the
// current engine's time to sleep). The engine alone waits ten substeps, and a slow swing would stop at the
// top of its arc.
const SLEEP_TIME: f64 = 0.5;
// The engine's sleep state of a tree just woken: awake for its minimum number of steps.
const AWAKE: i32 = -(1 + MINAWAKE as i32);
type Event = (usize, Option<usize>, f64);

#[derive(Clone)]
struct Pin {
    body: usize,
    local: P3,
    world: P3,
}

/// A joint's motor in the model: its actuator and what it is set to.
struct Motored {
    joint: usize,
    actuator: usize,
    kind: crate::MotorKind,
    target: f64,
}

// A motor is a servo (a position actuator) or a drive (a velocity actuator) on its joint, its force clamped to
// the motor's limit. A servo pushes its whole limit this far off its target, as a hobby servo does; a drive
// this far off its speed. Both are as stiff as that, or as a substep holds when that is stiffer: their gains
// come from the engine's own measure of the inertia the joint moves (acc0), so the stiffest servo still swings
// at a quarter radian per substep.
const SERVO_BAND_HINGE: f64 = 3.0 * std::f64::consts::PI / 180.0;
const SERVO_BAND_SLIDER: f64 = 0.5;
const DRIVE_BAND_HINGE: f64 = 0.5;
const DRIVE_BAND_SLIDER: f64 = 5.0;
const MOTOR_SWING: f64 = 0.25;

fn motor_name(j: usize) -> String { format!("motor_{j}") }

// The motor of `asm.joints[j]`: unit gains; DynWorld::tune_motors sets them from the compiled model.
fn motor_spec(j: usize, motor: &Motor) -> ActuatorSpec {
    let mut a = ActuatorSpec { name: Some(motor_name(j)), ctrllimited: 0, forcelimited: 1, forcerange: [-motor.limit, motor.limit], ..ActuatorSpec::on(TRN_JOINT, &joint_name(j)) };
    match motor.kind {
        crate::MotorKind::Position => a.set_to_position(1.0, None, None, None, 0.0),
        crate::MotorKind::Speed => a.set_to_velocity(1.0),
    }.expect("a motor's shortcut takes unit gains");
    a
}

#[derive(Clone)]
struct Frozen {
    body: usize,
    pos: P3,
    quat: [f64; 4],
}

struct Grab {
    body: usize,
    local: P3,
    /// Where the hand is at the start of the step and where it ends it.
    from: P3,
    at: P3,
    hertz: f64,
    /// The most the hand pulls with.
    max_force: f64,
    velocity: P3,
    linear_damping: f64,
    angular_damping: f64,
    dry_torque: f64,
}

/// A grab on a held part: the held point in the ground's frame. A drag moves the ground, which carries
/// every held part, so that point is under the hand at the end of each step.
struct Carry {
    body: usize,
    local: P3,
}

/// The engine found its state blown up in a substep and put everything back at the print pose: when (the
/// sim's time before that substep) and on which part it saw it first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reset { pub time: f64, pub part: Option<usize> }

/// One compiled model and its live state. Part indices always refer to the assembly.
pub struct DynWorld {
    model: Model,
    data: Data,
    spec: ModelSpec,
    /// The field geoms (by name) and their fields, attached to each model compiled from the spec.
    fields: Vec<(String, Arc<dyn Sdf>)>,
    assembly: Assembly,
    bodies: Vec<usize>,
    origins: Vec<P3>,
    joints: Vec<Option<usize>>,
    motors: Vec<Motored>,
    /// What the panel reads, as last measured while its tree was awake (a sleeping tree has no rows to read):
    /// per assembly joint the force through it (N), per motor what it pushes (N mm, a slider's N), per
    /// string its tension (N).
    loads: Vec<Option<f64>>,
    efforts: Vec<f64>,
    tensions: Vec<f64>,
    /// Per model body, the share of its part's mass it carries (a part that closes loops shares it with
    /// the bodies of its loop joints).
    shares: Vec<f64>,
    /// The welds that close loops (equality ids).
    loops: Vec<usize>,
    /// Per tree, how long it has been under the sleep speed (seconds).
    still: Vec<f64>,
    anchor_joints: Vec<(usize, Friction)>,
    geom_parts: Vec<Option<usize>>,
    pins: Vec<Pin>,
    grab: Option<Grab>,
    carry: Option<Carry>,
    substeps: u32,
    touching: Vec<(usize, usize)>,
    next_touching: Vec<(usize, usize)>,
    resets: Vec<Reset>,
    speed_scale: Vec<f64>,
    impulse: Vec<f64>,
    frozen: Vec<Frozen>,
    ground: usize,
    ground_velocity: P3,
    ground_spin: P3,
    ground_left: f64,
    begins: Vec<Event>,
    hit_events: Vec<Event>,
    pub gravity: P3,
}

fn root(parents: &mut [usize], i: usize) -> usize {
    if parents[i] != i {
        parents[i] = root(parents, parents[i]);
    }
    parents[i]
}

fn join(parents: &mut [usize], a: usize, b: usize) -> bool {
    let a = root(parents, a);
    let b = root(parents, b);
    if a == b { false } else { parents[b] = a; true }
}

fn body_name(i: usize) -> String { format!("part_{i}") }
fn string_name(i: usize) -> String { format!("string_{i}") }
fn loop_name(i: usize) -> String { format!("loop_{i}") }
fn joint_name(i: usize) -> String { format!("joint_{i}") }
fn triple(values: &[f64], i: usize) -> P3 { values[3*i..3*i+3].try_into().unwrap() }
fn quaternion(values: &[f64], i: usize) -> [f64; 4] { values[4*i..4*i+4].try_into().unwrap() }
fn rotated(q: [f64; 4], p: P3) -> P3 {
    let mut out = [0.0; 3];
    util_spatial::rot_vec_quat(&mut out, &p, &q);
    out
}
fn unrotated(q: [f64; 4], p: P3) -> P3 { rotated([q[0], -q[1], -q[2], -q[3]], p) }
fn inertia_about(inertia: [[f64; 3]; 3], axis: P3) -> f64 {
    (0..3).map(|i| (0..3).map(|j| axis[i]*inertia[i][j]*axis[j]).sum::<f64>()).sum()
}
fn reach(asm: &Assembly, body: usize) -> f64 {
    let b = &asm.bodies[body];
    b.tubes.iter().flat_map(|(pts, r)| pts.iter().map(move |p| len(sub(*p, b.mass.com)) + r))
        .chain(b.rods.iter().flat_map(|(a, c, r)| [len(sub(*a, b.mass.com)) + r, len(sub(*c, b.mass.com)) + r]))
        .chain(b.hulls.iter().flatten().map(|p| len(sub(*p, b.mass.com))))
        .chain(b.balls.iter().map(|(p,r)| len(sub(*p, b.mass.com)) + r))
        .fold(0.0, f64::max)
}
fn contact_hertz(asm: &Assembly) -> f64 {
    if asm.contact_hertz > 0.0 { asm.contact_hertz } else { CONTACT_HERTZ }
}
// As stiff as the engine allows (impedance 0.9999 at any violation). A loop's closing weld, so the solve carries
// the loop's forces and the closure projection after it has little left to take; the loop joint's limits and
// friction alike, so they hold as a tree joint's do.
const STIFF_SOLIMP: [f64; NIMP] = [0.9999, 0.9999, 0.001, 0.5, 2.0];
// A joint's friction. At the engine's default (impedance 0.9, time constant 20 ms) friction is a soft constraint, and
// a joint under load creeps through it instead of standing still, at about load * (1-d)/d * time constant/2: some
// 10 mm/s under gravity, so a posed limb or a car's wheels take a second to stop. The time constant at its floor of
// two substeps (joint_spec) and this impedance bring that to some 0.05 mm/s; the impedance alone would need to go
// higher, and the stiffer the row the harder the solve (the dragon's worst step doubles at 0.9999).
const FRICTION_SOLIMP: [f64; NIMP] = [0.99, 0.99, 0.001, 0.5, 2.0];

// The joint of `asm.joints[j]` at `pos` in its child body's frame, its axis times `sign`.
fn joint_spec(asm: &Assembly, j: usize, pos: P3, sign: f64, dt: f64) -> JointSpec {
    let source = &asm.joints[j];
    let mut joint = JointSpec {
        name: Some(joint_name(j)), pos, axis: scale(norm(source.axis),sign),
        frictionloss: source.friction.torque, solimp_friction: FRICTION_SOLIMP, solref_friction: [2.0*dt,1.0], solref_limit: [2.0*dt,1.0], limited: 0,
        ..JointSpec::default()
    };
    match source.kind {
        JointKind::Revolute { lower,upper } => {
            joint.limited = i32::from(!source.kind.turns_freely());
            joint.range = [lower,upper];
        }
        JointKind::Prismatic { lower,upper } => { joint.r#type = JNT_SLIDE; joint.limited = 1; joint.range = [lower,upper]; }
        JointKind::Spherical { cone, .. } => {
            // A ball joint has no independent twist range.
            joint.r#type = JNT_BALL; joint.limited = 1; joint.range = [0.0,cone];
        }
        JointKind::Flex { stiffness,damping,one_way } => {
            joint.r#type = if one_way { JNT_HINGE } else { JNT_BALL };
            let about = |axis| {
                let ia = inertia_about(asm.bodies[source.a].mass.inertia,axis);
                let ib = inertia_about(asm.bodies[source.b].mass.inertia,axis);
                1.0 / (1.0/ia + 1.0/ib)
            };
            let axis = norm(source.axis);
            let inertia = if one_way { about(axis) } else { let (u,v)=basis(axis); 0.5*(about(u)+about(v)) };
            joint.stiffness[0] = stiffness*1e6;
            joint.damping[0] = 2.0*damping*(joint.stiffness[0]*inertia).sqrt();
            joint.springref = 0.0;
        }
        JointKind::Weld => unreachable!(),
    }
    joint
}

fn timeconst(hertz: f64, dt: f64) -> f64 { (1.0 / (std::f64::consts::TAU * hertz)).max(2.0 * dt) }
// Contacts are found at touch (no margin, no gap). The assembly's contact margin is for an engine that
// needs contacts ahead of time to keep thin fast links from passing through each other; here speculative
// contacts do that (a geom that moves more than its size in a substep is swept), and a detection gap
// would only add contacts that never act (on a hanging mail, 1700 found for 34 that act).
// A link collides as capsules along its centreline, kept to the points it needs to stay within this of the
// printed one (mm): fewer than half the capsules (the mail demo's 1148 become 492), and a chain's step
// fits the view's time. The chords cut the links' rounded corners by up to this much, so two interlocked
// links' solids may overlap by up to 0.3 mm at a corner; links stay linked through hangs and drags.
const TUBE_TOLERANCE: f64 = 0.3;

// Two parts' fields meet where the engine's search finds them (collision_sdf::sdf): each collision it walks from
// where they last touched, from the middle of their common box and from SDF_INITPOINTS more points in each cube
// of the thinner part's thickness along the box's longest side, each walk until it settles at the deepest point
// of their overlap (its step under a thousandth of the box's size); from each touch it finds how far it reaches
// along their surfaces (a face's or a line's ends), and keeps as many contacts (the deepest and those farthest
// apart). A walk still going after SDF_ITERATIONS steps carries on in the next collision
// (Data::sdf_walks_bounded, logged at the first); the parts rarely need it: of the walks of a pin pulled into
// its hole's wall, 5 in 123 thousand, none of a roller pulled into its bearing's ring or of a ball at rest in its
// cup. Per 240 Hz step at the 4 substeps fields run at (runner::dynamics_world): a bearing's rings and rollers
// 0.94 ms, a ball in its cup 0.12 ms, a pin in its hole 0.13 ms; the roller pulled into its ring goes at most
// 0.044 mm into it on Fast's grids and 0.057 mm on Exact's, a ball rests in its cup within 3.5 s. A hull meets a
// field through its faces, one start on each walked at most SDF_ITERATIONS steps to its deepest corner.
const SDF_INITPOINTS: i32 = 8;
const SDF_ITERATIONS: i32 = 64;
// What meets what (contype and conaffinity bits). The floor, tubes and the drape ball are SOLID; rods meet rods
// and the floor; a part's hulls meet the floor, tubes, hulls and fields; fields meet fields and hulls. A part
// with a field meets the others through it, and its own hulls only the floor and tubes (they would fill its
// hollows).
const SOLID: i32 = 1;
const ROD: i32 = 2;
const FIELD: i32 = 4;
const HULL: i32 = 8;

// A part's field (its distance, in the model frame) as the engine's SDF of its field geom, in the geom's
// frame: the model frame moved to the centre of the field's bounds. The field gives the distance and the way
// it grows from one lookup, so the engine's walk asks it once per point.
struct FieldSdf {
    field: Arc<dyn DistanceField>,
    centre: P3,
}

impl FieldSdf {
    fn at(&self, p: &[f64; 3]) -> (f64, [f64; 3]) {
        let (d, n) = self.field.distance(makepad_box3d::math_functions::vec3((p[0] + self.centre[0]) as f32, (p[1] + self.centre[1]) as f32, (p[2] + self.centre[2]) as f32));
        (d as f64, [n.x as f64, n.y as f64, n.z as f64])
    }
}

impl Sdf for FieldSdf {
    fn distance(&self, point: &[f64; 3]) -> f64 {
        self.at(point).0
    }
    fn gradient(&self, gradient: &mut [f64; 3], point: &[f64; 3]) {
        *gradient = self.at(point).1;
    }
    fn distance_gradient(&self, gradient: &mut [f64; 3], point: &[f64; 3]) -> f64 {
        let (d, g) = self.at(point);
        *gradient = g;
        d
    }
}

// The builder takes no fields: a field geom compiles as a box over the field's bounds (the broad phase's
// extent) and becomes its field once compiled.
fn attach_fields(model: &mut Model, fields: &[(String, Arc<dyn Sdf>)]) {
    for (name, field) in fields {
        let geom = name::name2id(model, OBJ_GEOM, name);
        assert!(geom >= 0, "field geom {name} is in the spec it compiled from");
        sdf::attach(model, geom as usize, field.clone());
    }
}

// The points of a closed centreline it needs to stay within `tol` of it (Douglas-Peucker, split first at the
// point farthest from the first one). Each half keeps its farthest point, so a loop stays a loop.
fn simplified(points: &[P3], tol: f64) -> Vec<P3> {
    let m = points.len();
    let at = |i: usize| points[i % m];
    let off = |p: P3, a: P3, b: P3| {
        let ab = sub(b, a);
        let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-12)).clamp(0.0, 1.0);
        len(sub(p, madd(a, ab, t)))
    };
    let far = (1..m).max_by(|&a, &b| len(sub(points[a], points[0])).total_cmp(&len(sub(points[b], points[0])))).unwrap_or(0);
    let mut keep = vec![false; m];
    keep[0] = true;
    keep[far] = true;
    let mut stack = vec![(0, far, true), (far, m, true)];
    while let Some((i, j, half)) = stack.pop() {
        let worst = (i + 1..j).map(|k| (k, off(at(k), at(i), at(j)))).max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((k, d)) = worst {
            if half || d > tol {
                keep[k % m] = true;
                stack.push((i, k, false));
                stack.push((k, j, false));
            }
        }
    }
    (0..m).filter(|&k| keep[k]).map(|k| points[k]).collect()
}

/// A field contact's impedance at first touch in the assembly's soft layer ([`Assembly::contact_layer`]); it
/// rises to the engine's 0.95 across the layer. At half, a part resting on fields sinks about a micron into them
/// under its load, past what a read's error takes away; softer lets a resting part creep.
const LAYER_D0: f64 = 0.5;

fn geom(asm: &Assembly, dt: f64, bits: i32) -> GeomSpec {
    let solimp = if bits == FIELD && asm.contact_layer > 0.0 { [LAYER_D0, 0.95, asm.contact_layer, 0.5, 2.0] } else { GeomSpec::default().solimp };
    GeomSpec {
        contype: bits, conaffinity: bits, condim: 3,
        friction: [CONTACT_MU, 0.0, 0.0], margin: 0.0, gap: 0.0, solimp,
        solref: [timeconst(contact_hertz(asm), dt), 1.0], ..GeomSpec::default()
    }
}
fn connect(a: String, b: Option<String>, local: P3, dt: f64) -> EqualitySpec {
    let mut data = [0.0; NEQDATA];
    data[..3].copy_from_slice(&local);
    EqualitySpec { name1: Some(a), name2: b, data, solref: [2.0*dt, 1.0], ..EqualitySpec::default() }
}

// Two parts' collision proxies can overlap where they printed: a hull takes in all that lies between a part's
// points, so it can reach into the part beside it where the real shapes keep their clearance. A contact would
// throw the part out from there, so a pair that overlaps gets a contact that pushes only past that depth (the
// engine's per-pair margin, negative here, with the gap that keeps the detection at touch). The pair still
// touches, so a lid still stops at its box.
//
// Hull facets don't follow a joint, so moving a part away from where it printed can sink it a little deeper before it
// comes clear, and a contact there would hold it. So the depth a pair gets is the deepest it goes on its way out, each
// way the hinge or slider between its parts goes, until they clear or the joint can go no further. A way that sinks it
// by more than its print depth again, and more than MOST_HUMP, is a stop and not a way out: a lid going down into its
// box. A ball joint, or a pair with more than one joint between its parts, keeps the print depth: there is no one line to follow.
const MOST_HUMP: f64 = 1.0;
const HINGE_STEP: f64 = 0.25*std::f64::consts::PI/180.0;
const SLIDER_STEP: f64 = 0.25;
fn print_overlaps(model: &Model, asm: &Assembly, dt: f64) -> Vec<PairSpec> {
    let mut data = io::make_data(model);
    forward::fwd_position(model,&mut data);
    let part_geom=|g: i32| name::id2name(model,OBJ_GEOM,g).filter(|n| n.starts_with(b"geom_")).map(|n| String::from_utf8_lossy(n).into_owned());
    let mut at_print = overlapping(&data);
    at_print.retain(|pair,_| part_geom(pair.0).is_some() && part_geom(pair.1).is_some());
    let mut deepest = at_print.clone();
    let joints: BTreeSet<usize> = at_print.keys().filter_map(|&pair| moving_joint(model,pair)).collect();
    for joint in joints {
        let adr=model.jnt_qposadr[joint] as usize;
        let rest=model.qpos0[adr];
        let step=if model.jnt_type[joint]==JNT_HINGE { HINGE_STEP } else { SLIDER_STEP };
        for sign in [1.0,-1.0] {
            let end=if model.jnt_limited[joint]!=0 { model.jnt_range[2*joint+usize::from(sign>0.0)] } else { rest+sign*std::f64::consts::PI };
            let steps=((end-rest).abs()/step).ceil() as usize;
            let mut out: BTreeMap<(i32,i32),f64> = at_print.iter().filter(|entry| moving_joint(model,*entry.0)==Some(joint)).map(|(&pair,&print)| (pair,print)).collect();
            let mut going: Vec<(i32,i32)> = out.keys().copied().collect();
            for k in 1..=steps {
                data.qpos[adr]=rest+(end-rest)*k as f64/steps as f64;
                core_smooth::kinematics(model,&mut data);
                collision_driver::collision(model,&mut data);
                let now=overlapping(&data);
                going.retain(|pair| {
                    let Some(&depth)=now.get(pair) else { return false };
                    let seen=out.get_mut(pair).expect("every pair going was put in out");
                    *seen=seen.min(depth);
                    let print=at_print[pair];
                    if print-*seen>(-print).max(MOST_HUMP) { *seen=print; return false; }
                    true
                });
                if going.is_empty() { break; }
            }
            data.qpos[adr]=rest;
            for (pair,depth) in out {
                let at=deepest.get_mut(&pair).expect("out only has pairs from at_print");
                *at=at.min(depth);
            }
        }
    }
    deepest.into_iter().filter_map(|((a,b),depth): ((i32,i32),f64)| {
        Some(PairSpec {
            geomname1:Some(part_geom(a)?),geomname2:Some(part_geom(b)?),margin:depth,gap:-depth,
            solref:[timeconst(contact_hertz(asm),dt),1.0],friction:[CONTACT_MU,CONTACT_MU,0.0,0.0,0.0],
            ..PairSpec::default()
        })
    }).collect()
}

// The pairs of parts' proxies that overlap in this pose, each at its deepest (negative).
fn overlapping(data: &Data) -> BTreeMap<(i32,i32),f64> {
    let mut deepest = BTreeMap::new();
    for c in &data.contact[..data.ncon as usize] {
        if c.dist<0.0 && c.geom.iter().all(|&g| g>=0) {
            let at=deepest.entry((c.geom[0],c.geom[1])).or_insert(0.0_f64);
            *at=at.min(c.dist);
        }
    }
    deepest
}

// The hinge or slider that alone moves a pair of proxies' bodies against each other.
fn moving_joint(model: &Model, pair: (i32,i32)) -> Option<usize> {
    let (a,b)=(model.geom_bodyid[pair.0 as usize] as usize,model.geom_bodyid[pair.1 as usize] as usize);
    let child=if model.body_parentid[a] as usize==b { a } else if model.body_parentid[b] as usize==a { b } else { return None };
    if model.body_jntnum[child]!=1 { return None; }
    let joint=model.body_jntadr[child] as usize;
    matches!(model.jnt_type[joint],JNT_HINGE|JNT_SLIDE).then_some(joint)
}

impl DynWorld {
    /// Spreads the engine's parallel work (the midphase and narrowphase, the islands' solves, tactile sensors)
    /// over `pool`, or keeps it on the stepping thread with None; the steps come out the same either way. The
    /// stepping thread helps and waits for each spread, so a world bound to a pool is never stepped on the UI
    /// thread.
    pub fn bind_pool(&mut self, pool: Option<Arc<dyn TaskPool>>) {
        makepad_dynamics::thread::bind_thread_pool(&mut self.data, pool);
    }

    pub fn new(asm: &Assembly, gravity: P3, floor: Option<f64>) -> Result<DynWorld, String> {
        let n = asm.bodies.len();
        if !gravity.iter().all(|x| x.is_finite()) { return Err("gravity must be finite".into()); }
        if floor.is_some() && len(gravity) == 0.0 { return Err("a floor needs a gravity direction".into()); }
        for b in &asm.bodies {
            if b.hulls.iter().flatten().any(|p| !p.iter().all(|v| v.is_finite())) { return Err("hull points must be finite".into()); }
            if !(b.mass.mass > 0.0 && b.mass.mass.is_finite()) { return Err("body mass must be finite and positive".into()); }
            if !b.mass.com.iter().chain(b.mass.inertia.iter().flatten()).all(|v| v.is_finite()) {
                return Err("body inertia and centre must be finite".into());
            }
        }
        for j in &asm.joints {
            if j.a >= n || j.b >= n || j.a == j.b { return Err("joint needs two different assembly bodies".into()); }
            if !matches!(j.kind, JointKind::Weld) && len(j.axis) == 0.0 { return Err("joint needs a nonzero axis".into()); }
        }
        for a in &asm.anchors {
            if a.body >= n { return Err("anchor body is outside the assembly".into()); }
        }
        check_strings(asm).map_err(|f| f.to_string())?;
        // Balls meet fields on box3d: runner::on_dynamics sends every piece with balls there (a support check's
        // pieces against the part), so no sim builds one here. Here the balls would meet nothing.
        if asm.bodies.iter().any(|b| b.field.is_some()) && asm.bodies.iter().any(|b| !b.balls.is_empty()) {
            return Err("balls meet fields on box3d (runner::on_dynamics routes them there)".into());
        }
        let substeps = if asm.substeps == 0 { 4 } else { asm.substeps };
        let dt = 1.0 / (crate::runner::HZ * substeps as f64);
        let mut spec = ModelSpec::default();
        spec.option.gravity = gravity;
        spec.option.timestep = dt;
        spec.option.cone = CONE_PYRAMIDAL;
        spec.option.enableflags |= ENBL_SPECULATIVE;
        spec.option.sdf_initpoints = SDF_INITPOINTS;
        spec.option.sdf_iterations = SDF_ITERATIONS;
        let mut fields: Vec<(String, Arc<dyn Sdf>)> = Vec::new();
        // Two parts on a joint touch away from it, as the current engine's do (their proxies leave the joint's
        // region out): the assembly says which pairs never touch (a joint's collide), so the engine's own
        // filter, which drops every contact between a body and its parent, is off.
        spec.option.disableflags |= DSBL_FILTERPARENT;
        if asm.sleep > 0.0 {
            // Each dof's speed is weighed by its length, so the tolerance is a speed in mm/s.
            spec.option.enableflags |= ENBL_SLEEP;
            spec.option.sleep_tolerance = asm.sleep;
        }
        spec.bodies.push(BodySpec { name: Some("ground".into()), mocap: true, ..BodySpec::default() });
        let mut welded: Vec<_> = (0..n).collect();
        for j in &asm.joints {
            if matches!(j.kind, JointKind::Weld) { join(&mut welded, j.a, j.b); }
        }
        let group: Vec<_> = (0..n).map(|i| root(&mut welded, i)).collect();
        let groups: Vec<_> = (0..n).filter(|&i| group[i] == i).collect();
        let mut anchors = BTreeMap::new();
        for (i, a) in asm.anchors.iter().enumerate() {
            if anchors.insert(group[a.body], i).is_some() {
                return Err("multiple anchors on one welded body need independent anchor constraints".into());
            }
        }
        let mut forest: Vec<_> = (0..=n).collect();
        for &g in anchors.keys() { join(&mut forest, n, g); }
        let mut tree = vec![false; asm.joints.len()];
        let mut edges: Vec<_> = (0..asm.joints.len()).collect();
        edges.sort_by_key(|&i| !matches!(asm.joints[i].kind, JointKind::Prismatic { .. }));
        for i in edges {
            let j = &asm.joints[i];
            if matches!(j.kind, JointKind::Weld) { continue; }
            if group[j.a] == group[j.b] { return Err("a joint inside a welded body is overconstrained".into()); }
            tree[i] = join(&mut forest, group[j.a], group[j.b]);
        }
        // A loop-closing joint is a real joint on an extra body hung from part a and welded to part b, so it
        // takes limits, friction and springs as a tree joint does. That body carries an equal share of b's
        // mass, inertia and centre of mass (the pair is b exactly): no constraint row meets a light body, whose
        // inverse weight would soften it. It has no geoms, and nothing outside DynWorld sees it.
        let mut sharers = vec![1usize; n];
        for (i,j) in asm.joints.iter().enumerate() {
            if !tree[i] && !matches!(j.kind, JointKind::Weld) { sharers[group[j.b]] += 1; }
        }
        let mut props = vec![(0.0,[0.0;3],[[0.0;3];3]); n];
        let mut adjacent = vec![Vec::new(); n];
        for (i,j) in asm.joints.iter().enumerate() {
            if tree[i] {
                adjacent[group[j.a]].push((group[j.b],i));
                adjacent[group[j.b]].push((group[j.a],i));
            }
        }
        let mut order = Vec::new();
        let mut visited = vec![false; n];
        let mut parent = vec![None; n];
        let mut queue: Vec<_> = anchors.keys().copied().collect();
        for &g in &queue { visited[g] = true; }
        loop {
            let mut next = 0;
            while next < queue.len() {
                let g = queue[next];
                next += 1;
                order.push(g);
                for &(other,j) in &adjacent[g] {
                    if !visited[other] {
                        visited[other] = true;
                        parent[other] = Some((g,j));
                        queue.push(other);
                    }
                }
            }
            queue.clear();
            let heaviest = groups.iter().copied().filter(|&g| !visited[g]).max_by(|&a,&b| {
                let mass = |g| asm.bodies.iter().enumerate().filter(|(i,_)| group[*i]==g).map(|(_,b)| b.mass.mass).sum::<f64>();
                mass(a).total_cmp(&mass(b))
            });
            let Some(g) = heaviest else { break };
            visited[g] = true;
            queue.push(g);
        }
        let mut hulls: Vec<(usize,usize)> = Vec::new();
        let mut spec_ids = vec![0; n];
        let mut origins = vec![[0.0;3]; n];
        for &g in &order {
            let origin = asm.bodies[g].mass.com;
            let mut mass = 0.0;
            let mut com = [0.0; 3];
            for (i,b) in asm.bodies.iter().enumerate().filter(|(i,_)| group[*i] == g) {
                mass += b.mass.mass;
                com = madd(com, b.mass.com, b.mass.mass);
                origins[i] = origin;
            }
            com = scale(com, 1.0/mass);
            let mut inertia = [[0.0;3];3];
            for (_,b) in asm.bodies.iter().enumerate().filter(|(i,_)| group[*i] == g) {
                let r = sub(b.mass.com, com);
                for i in 0..3 { for k in 0..3 {
                    inertia[i][k] += b.mass.inertia[i][k] + b.mass.mass * (if i == k { dot(r,r) } else { 0.0 } - r[i]*r[k]);
                }}
            }
            props[g] = (mass,com,inertia);
            let part = 1.0/sharers[g] as f64;
            let mut body = BodySpec {
                name: Some(body_name(g)), mass: mass*part, ipos: sub(com,origin),
                fullinertia: [inertia[0][0],inertia[1][1],inertia[2][2],inertia[0][1],inertia[0][2],inertia[1][2]].map(|v| v*part),
                pos: origin, ..BodySpec::default()
            };
            if let Some((p,j)) = parent[g] {
                body.parent = spec_ids[p];
                body.pos = sub(origin, asm.bodies[p].mass.com);
                let source = &asm.joints[j];
                let sign = if group[source.b] == g { 1.0 } else { -1.0 };
                let joint = joint_spec(asm,j,sub(source.pivot,origin),sign,dt);
                body.joints.push(joint);
            } else if let Some(&a) = anchors.get(&g) {
                let anchor = &asm.anchors[a];
                body.parent = 1;
                if let AnchorKind::Pin(axis) = anchor.kind {
                    body.joints.push(JointSpec { name: Some(format!("anchor_{a}")), pos: sub(anchor.pivot,origin), axis: norm(axis), limited: 0, frictionloss: anchor.friction.torque, solimp_friction: FRICTION_SOLIMP, solref_friction: [2.0*dt,1.0], ..JointSpec::default() });
                }
            } else {
                body.joints.push(JointSpec { name: Some(format!("free_{g}")), r#type: JNT_FREE, limited: 0, ..JointSpec::default() });
            }
            for (i,b) in asm.bodies.iter().enumerate().filter(|(i,_)| group[*i]==g) {
                let mut serial = 0;
                let mut capsule = |a:P3, c:P3, radius:f64, bits| -> Result<(),String> {
                    if !(radius>0.0 && radius.is_finite()) || !a.iter().chain(c.iter()).all(|v|v.is_finite()) {
                        return Err("capsule needs finite endpoints and a positive radius".into());
                    }
                    let a = sub(a,origin); let c = sub(c,origin);
                    body.geoms.push(GeomSpec {
                        name: Some(format!("geom_{i}_{serial}")), r#type: GEOM_CAPSULE, size: [radius,0.0,0.0],
                        fromto: [a[0],a[1],a[2],c[0],c[1],c[2]], ..geom(asm,dt,bits)
                    });
                    serial += 1;
                    Ok(())
                };
                for (points,r) in &b.tubes {
                    if points.len() < 3 { return Err("a closed tube needs at least three points".into()); }
                    let points = simplified(points, TUBE_TOLERANCE);
                    for k in 0..points.len() { capsule(points[k],points[(k+1)%points.len()],*r,SOLID)?; }
                }
                for &(a,c,r) in &b.rods { capsule(a,c,r,ROD)?; }
                let hull_bits = if b.field.is_some() {
                    GeomSpec { contype: 0, ..geom(asm,dt,SOLID) }
                } else {
                    GeomSpec { contype: HULL, conaffinity: SOLID|HULL, ..geom(asm,dt,SOLID) }
                };
                for (k,h) in b.hulls.iter().enumerate() {
                    let name = format!("mesh_{i}_{k}");
                    let vert = solid_hull(h).iter().flat_map(|p| { let p = sub(*p,origin); [p[0] as f32,p[1] as f32,p[2] as f32] }).collect();
                    hulls.push((i,k));
                    spec.meshes.push(MeshSpec { name: Some(name.clone()), vert, ..MeshSpec::default() });
                    body.geoms.push(GeomSpec {
                        name: Some(format!("geom_{i}_{serial}")), r#type: GEOM_MESH, meshname: Some(name), ..hull_bits.clone()
                    });
                    serial += 1;
                }
                if let Some(field) = &b.field {
                    let bounds = field.0.bounds();
                    let lo = [bounds.lower_bound.x as f64, bounds.lower_bound.y as f64, bounds.lower_bound.z as f64];
                    let hi = [bounds.upper_bound.x as f64, bounds.upper_bound.y as f64, bounds.upper_bound.z as f64];
                    if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) || (0..3).any(|k| !(hi[k] > lo[k])) {
                        return Err(format!("body {}: a field needs finite bounds with volume", b.name));
                    }
                    let centre = scale(add(lo,hi),0.5);
                    let name = format!("geom_{i}_{serial}");
                    body.geoms.push(GeomSpec { name: Some(name.clone()), r#type: GEOM_BOX, pos: sub(centre,origin), size: scale(sub(hi,lo),0.5), contype: FIELD, conaffinity: FIELD|HULL, ..geom(asm,dt,FIELD) });
                    fields.push((name, Arc::new(FieldSdf { field: field.0.clone(), centre })));
                }
                // Balls meet only fields, and run on box3d beside one (above): alone, they meet nothing and are omitted.
            }
            spec_ids[g] = spec.bodies.len();
            spec.bodies.push(body);
        }
        for (i,j) in asm.joints.iter().enumerate() {
            if group[j.a] == group[j.b] { continue; }
            if !j.collide {
                spec.excludes.push(ExcludeSpec { bodyname1:Some(body_name(group[j.a])),bodyname2:Some(body_name(group[j.b])),..ExcludeSpec::default() });
            }
            if !tree[i] {
                let (mass,com,inertia) = props[group[j.b]];
                let part = 1.0/sharers[group[j.b]] as f64;
                let mut body = BodySpec {
                    name: Some(loop_name(i)), parent: spec_ids[group[j.a]], pos: sub(j.pivot,origins[j.a]),
                    mass: mass*part, ipos: sub(com,j.pivot),
                    fullinertia: [inertia[0][0],inertia[1][1],inertia[2][2],inertia[0][1],inertia[0][2],inertia[1][2]].map(|v| v*part),
                    ..BodySpec::default()
                };
                // Its limit and friction rows take their softness from the tree's inverse weights, which see the
                // shared body but not the loop's parts that move with it: as stiff as the closing weld.
                body.joints.push(JointSpec { solimp_limit: STIFF_SOLIMP, solimp_friction: STIFF_SOLIMP, ..joint_spec(asm,i,[0.0;3],1.0,dt) });
                spec.bodies.push(body);
                // the weld's anchor is on part b; its relative pose is taken at the rest pose
                let mut data = [0.0;NEQDATA];
                data[..3].copy_from_slice(&sub(j.pivot,origins[j.b]));
                data[10] = 1.0;
                spec.equalities.push(EqualitySpec {
                    name: Some(loop_name(i)), r#type: EQ_WELD, name1: Some(loop_name(i)), name2: Some(body_name(group[j.b])),
                    data, solref: [2.0*dt,1.0], solimp: STIFF_SOLIMP, ..EqualitySpec::default()
                });
            }
        }
        // A string is a site at each of its points and one tendon through them, limited to its length: taut,
        // it holds as stiffly as a loop's weld; slack, it exerts nothing (no spring, no damping). Its tendon's
        // id is its index, as strings are the only tendons.
        for (s,string) in asm.strings.iter().enumerate() {
            let mut path = Vec::new();
            for (k,(part,at)) in string.points().enumerate() {
                let site = format!("{}_{k}",string_name(s));
                spec.bodies[spec_ids[group[part]]].sites.push(SiteSpec { name: Some(site.clone()), pos: sub(at,origins[part]), ..SiteSpec::default() });
                path.push(WrapSpec::site(&site));
            }
            spec.tendons.push(TendonSpec {
                name: Some(string_name(s)), limited: 1, range: [0.0,string.length],
                solref_limit: [2.0*dt,1.0], solimp_limit: STIFF_SOLIMP, path, ..TendonSpec::default()
            });
        }
        // A motor drives a hinge or a slider, tree joint or loop joint alike: both are named for the assembly's joint.
        let mut motors = Vec::new();
        for (i,j) in asm.joints.iter().enumerate() {
            let Some(motor) = &j.motor else { continue };
            if !matches!(j.kind, JointKind::Revolute { .. } | JointKind::Prismatic { .. }) { return Err(format!("joint {}: a motor drives a hinge or a slider", j.name)); }
            if !(motor.limit > 0.0 && motor.limit.is_finite()) { return Err(format!("joint {}: a motor's limit must be finite and positive", j.name)); }
            motors.push(Motored { joint: i, actuator: spec.actuators.len(), kind: motor.kind, target: 0.0 });
            spec.actuators.push(motor_spec(i, motor));
        }
        if let Some(level) = floor {
            let up = scale(norm(gravity),-1.0);
            let mut quat = [0.0;4];
            util_spatial::quat_z2vec(&mut quat,&up);
            spec.bodies[0].geoms.push(GeomSpec { name:Some("floor".into()),r#type:GEOM_PLANE,pos:scale(up,level),quat,size:[0.0,0.0,1.0],..geom(asm,dt,SOLID|ROD) });
        }
        if let Some((centre,radius)) = asm.ball {
            spec.bodies[0].geoms.push(GeomSpec { name:Some("drape_ball".into()),pos:centre,size:[radius,0.0,0.0],..geom(asm,dt,SOLID) });
        }
        let mut model = builder::compile(&spec).map_err(|e| hull_error(&spec,&hulls).unwrap_or(e))?;
        attach_fields(&mut model,&fields);
        let overlaps = print_overlaps(&model,asm,dt);
        if !overlaps.is_empty() {
            spec.pairs.extend(overlaps);
            model = builder::compile(&spec).map_err(|e| hull_error(&spec,&hulls).unwrap_or(e))?;
            attach_fields(&mut model,&fields);
        }
        let data = io::make_data(&model);
        let bodies: Vec<_> = group.iter().map(|&g| name::name2id(&model,OBJ_BODY,&body_name(g)) as usize).collect();
        let joints: Vec<Option<usize>> = (0..asm.joints.len()).map(|i| {
            let id=name::name2id(&model,OBJ_JOINT,&joint_name(i)); (id>=0).then_some(id as usize)
        }).collect();
        let mut shares = vec![1.0; model.nbody];
        for &g in &groups { shares[name::name2id(&model,OBJ_BODY,&body_name(g)) as usize] = 1.0/sharers[g] as f64; }
        for (i,j) in asm.joints.iter().enumerate() {
            let id = name::name2id(&model,OBJ_BODY,&loop_name(i));
            if id >= 0 { shares[id as usize] = 1.0/sharers[group[j.b]] as f64; }
        }
        let anchor_joints = asm.anchors.iter().enumerate().filter_map(|(i,a)| {
            let id=name::name2id(&model,OBJ_JOINT,&format!("anchor_{i}")); (id>=0).then_some((id as usize,a.friction))
        }).collect();
        let (loads,efforts,tensions) = (joints.iter().map(|j| j.map(|_| 0.0)).collect(),vec![0.0;motors.len()],vec![0.0;asm.strings.len()]);
        let mut world = DynWorld { model,data,spec,fields,assembly:asm.clone(),bodies,origins,joints,motors,loads,efforts,tensions,shares,loops:Vec::new(),still:Vec::new(),anchor_joints,
            geom_parts:Vec::new(),pins:Vec::new(),grab:None,carry:None,substeps,touching:Vec::new(),next_touching:Vec::new(),resets:Vec::new(),speed_scale:Vec::new(),impulse:Vec::new(),frozen:Vec::new(),ground:0,ground_velocity:[0.0;3],ground_spin:[0.0;3],ground_left:0.0,begins:Vec::new(),hit_events:Vec::new(),gravity };
        world.configure();
        world.tune_motors();
        forward::forward(&world.model,&mut world.data);
        // paused at the print pose, the panel reads what holds it there
        core_smooth::rne_post_constraint(&world.model,&mut world.data);
        world.read();
        Ok(world)
    }

    fn configure(&mut self) {
        self.ground = name::name2id(&self.model, OBJ_BODY, "ground") as usize;
        self.speed_scale.resize(self.model.nbody, 1.0);
        self.impulse.resize(self.model.nv,0.0);
        self.geom_parts = vec![None; self.model.ngeom];
        self.loops = (0..self.assembly.joints.len()).filter_map(|i| {
            let id = name::name2id(&self.model,OBJ_EQUALITY,&loop_name(i)); (id>=0).then_some(id as usize)
        }).collect();
        // A new model starts awake; no tree may sleep before SLEEP_TIME.
        self.still = vec![0.0; self.model.ntree];
        self.model.tree_sleep_policy.fill(SLEEP_NEVER);

        for (i,b) in self.assembly.bodies.iter().enumerate() {
            let count = b.tubes.iter().map(|(p,_)| p.len()).sum::<usize>() + b.rods.len() + b.hulls.len() + usize::from(b.field.is_some());
            for k in 0..count {
                let id = name::name2id(&self.model,OBJ_GEOM,&format!("geom_{i}_{k}"));
                if id >= 0 { self.geom_parts[id as usize]=Some(i); }
            }
        }
        for j in 0..self.model.njnt {
            if self.model.jnt_type[j] == JNT_FREE {
                let adr = self.model.jnt_dofadr[j] as usize;
                let body = self.model.jnt_bodyid[j] as usize;
                let whole = 1.0/self.shares[body];
                for k in 0..3 { self.model.dof_damping[adr+k] = AIR_LINEAR_DAMPING*self.model.body_mass[body]*whole; }
                for k in 0..3 { self.model.dof_damping[adr+3+k] = AIR_ANGULAR_DAMPING*self.model.body_inertia[3*body+k]*whole; }
            }
        }
    }

    pub fn name(&self) -> &'static str { "dynamics" }
    pub fn time(&self) -> f64 { self.data.time }
    /// Gravity from now on, mm/s^2 (rebuilds keep it). It wakes everything, as nothing rests under a new pull.
    pub fn set_gravity(&mut self, g: P3) {
        let all: Vec<usize> = (0..self.model.nbody).collect();
        self.wake(&all);
        self.gravity = g;
        self.model.opt.gravity = g;
        self.spec.option.gravity = g;
    }
    pub fn rest_com(&self, body: usize) -> P3 { self.assembly.bodies[body].mass.com }
    pub fn kept_out(&self) -> usize { 0 }

    fn world_point(&self, body: usize, local: P3) -> P3 {
        add(triple(&self.data.xpos,body),rotated(quaternion(&self.data.xquat,body),local))
    }
    fn local_point(&self, body: usize, point: P3) -> P3 {
        unrotated(quaternion(&self.data.xquat,body),sub(point,triple(&self.data.xpos,body)))
    }
    pub fn poses(&self) -> Vec<Pose> {
        self.bodies.iter().enumerate().map(|(i,&body)| {
            let q=quaternion(&self.data.xquat,body);
            Pose { com:self.world_point(body,sub(self.rest_com(i),self.origins[i])),rot:[q[1],q[2],q[3],q[0]] }
        }).collect()
    }
    /// Each string as it hangs now.
    pub fn strings(&self) -> Vec<StringPath> {
        self.assembly.strings.iter().enumerate().map(|(t,string)| {
            let adr = self.data.ten_wrapadr[t] as usize;
            let num = self.data.ten_wrapnum[t] as usize;
            StringPath {
                points: (adr..adr+num).map(|k| triple(&self.data.wrap_xpos,k)).collect(),
                slack: (string.length-self.data.ten_length[t]).max(0.0),
                tension: self.tensions[t],
            }
        }).collect()
    }
    /// Per assembly joint, the force through it (N); none for a weld (its parts are one body).
    pub fn loads(&self) -> Vec<Option<f64>> { self.loads.clone() }
    /// Per motor, in [`motors`](Self::motors)' order, the torque it pushes with (N mm; a slider's force, N).
    pub fn efforts(&self) -> Vec<f64> { self.efforts.clone() }
    // The readings of the substep just taken: a joint's force is what its body's parent puts through it, a
    // motor's what its actuator pushes, a string's the force of its tendon's limit row (none when slack).
    fn read(&mut self) {
        let (m,d)=(&self.model,&self.data);
        let awake=|body: usize| { let t=m.body_treeid[body]; t<0 || d.tree_asleep[t as usize]<0 };
        for (load,j) in self.loads.iter_mut().zip(&self.joints) {
            let Some(j)=*j else { continue };
            let body=m.jnt_bodyid[j] as usize;
            if awake(body) { *load=Some(len(triple(&d.cfrc_int,2*body+1))/NEWTON); }
        }
        for (effort,motor) in self.efforts.iter_mut().zip(&self.motors) {
            if self.joints[motor.joint].is_some_and(|j| awake(m.jnt_bodyid[j] as usize)) { *effort=d.actuator_force[motor.actuator]/NEWTON; }
        }
        for (t,(tension,string)) in self.tensions.iter_mut().zip(&self.assembly.strings).enumerate() {
            if !string.points().any(|(part,_)| awake(self.bodies[part])) { continue; }
            *tension=(0..d.nefc as usize).filter(|&r| d.efc_type[r]==CNSTR_LIMIT_TENDON && d.efc_id[r]==t as i32).map(|r| d.efc_force[r]).sum::<f64>()/NEWTON;
        }
    }
    fn velocity(&self, body: usize, point: P3) -> P3 {
        let mut v=[0.0;6];
        core_util::object_velocity(&self.model,&self.data,OBJ_BODY,body,&mut v,0);
        add([v[3],v[4],v[5]],cross([v[0],v[1],v[2]],sub(point,triple(&self.data.xipos,body))))
    }
    pub fn max_speed(&self) -> f64 {
        self.bodies.iter().enumerate().map(|(i,&body)| len(self.velocity(body,self.world_point(body,sub(self.rest_com(i),self.origins[i]))))).fold(0.0,f64::max)
    }
    pub fn max_angular_speed(&self) -> f64 {
        self.bodies.iter().map(|&b| {
            let mut v=[0.0;6]; core_util::object_velocity(&self.model,&self.data,OBJ_BODY,b,&mut v,0);
            len([v[0],v[1],v[2]])
        }).fold(0.0,f64::max)
    }

    fn update_friction(&mut self) {
        for (i,j) in self.joints.iter().enumerate() {
            let Some(j)=*j else { continue };
            let source=&self.assembly.joints[i];
            let angle=self.data.qpos[self.model.jnt_qposadr[j] as usize];
            let torque=source.friction.torque+source.brake.as_ref().map_or(0.0,|b| b.at(angle));
            let force=len(triple(&self.data.cfrc_int,2*self.model.jnt_bodyid[j] as usize+1));
            let loss=torque+source.friction.mu*source.friction.radius*force;
            let adr=self.model.jnt_dofadr[j] as usize;
            let count=if self.model.jnt_type[j]==JNT_BALL {3} else {1};
            self.model.dof_frictionloss[adr..adr+count].fill(loss);
        }
        for &(j,f) in &self.anchor_joints {
            let force=len(triple(&self.data.cfrc_int,2*self.model.jnt_bodyid[j] as usize+1));
            self.model.dof_frictionloss[self.model.jnt_dofadr[j] as usize]=f.torque+f.mu*f.radius*force;
        }
    }

    pub fn step(&mut self, dt: f64) {
        assert!(dt.is_finite() && dt>0.0,"step needs a finite positive duration");
        let h=dt/self.substeps as f64;
        if h != self.model.opt.timestep {
            self.model.opt.timestep=h;
            self.spec.option.timestep=h;
            for r in self.model.geom_solref.chunks_exact_mut(NREF) { r[0]=timeconst(contact_hertz(&self.assembly),h); }
            for r in self.model.pair_solref.chunks_exact_mut(NREF) { r[0]=timeconst(contact_hertz(&self.assembly),h); }
            for r in self.model.eq_solref.chunks_exact_mut(NREF) { r[0]=2.0*h; }
            for r in self.model.dof_solref.chunks_exact_mut(NREF) { r[0]=2.0*h; }
            self.tune_motors();
        }
        self.begins.clear();
        self.hit_events.clear();
        // With every tree asleep and nothing to move one (no hand, a still ground), a step changes nothing but
        // the time: the engine would only find again that everything rests.
        if self.model.opt.enableflags & ENBL_SLEEP != 0 && self.data.ntree_awake == 0 && self.grab.is_none() && self.ground_left <= 0.0 {
            for _ in 0..self.substeps { self.data.time += h; }
            return;
        }
        let hand=self.grab.as_ref().map(|g| (g.from,sub(g.at,g.from)));
        for k in 0..self.substeps {
            let time=self.data.time;
            self.advance_ground(h);
            self.gate_sleep(h);
            self.update_friction();
            forward::step1(&self.model,&mut self.data);
            // The hand moves from where it was to where it is over the step, as the current engine's target body does.
            if let Some((from,travel))=hand { self.pull(madd(from,travel,k as f64/self.substeps as f64),scale(travel,1.0/dt),h); }
            if k==0 { self.damp_grip(dt,h); }
            forward::step2(&self.model,&mut self.data);
            self.note_resets(time);
            core_smooth::rne_post_constraint(&self.model,&mut self.data);
            self.contacts();
            core_smooth::com_vel(&self.model,&mut self.data);
            self.limit_speed();
            self.close_loops();
        }
        if let Some(g)=&mut self.grab { g.from=g.at; }
        self.read();
        // The integrator advances qpos after computing xpos; expose the completed step's pose.
        core_smooth::kinematics(&self.model,&mut self.data);
        core_smooth::com_pos(&self.model,&mut self.data);
        // and the strings' paths through it
        if self.model.ntendon>0 { core_smooth::tendon(&self.model,&mut self.data); }
        core_smooth::com_vel(&self.model,&mut self.data);
    }

    // The engine goes back to the print pose by itself when its state blows up, and says so only in a warning
    // it counts: a piece that teleports home must not pass for one at rest.
    fn note_resets(&mut self, time: f64) {
        for kind in [WARN_BADQPOS,WARN_BADQVEL,WARN_BADQACC] {
            let warning=&mut self.data.warning[kind as usize];
            if warning.number==0 { continue; }
            warning.number=0;
            let at=warning.lastinfo;
            let dof=if kind==WARN_BADQPOS {
                (0..self.model.njnt).rev().find(|&j| self.model.jnt_qposadr[j]<=at).map_or(0,|j| self.model.jnt_dofadr[j] as usize)
            } else { at as usize };
            let body=self.model.dof_bodyid[dof] as usize;
            self.resets.push(Reset {time,part:self.bodies.iter().position(|&b| b==body)});
        }
    }
    /// Every reset of the engine so far.
    pub fn resets(&self) -> &[Reset] { &self.resets }

    fn contacts(&mut self) {
        self.next_touching.clear();
        for c in &self.data.contact[..self.data.ncon as usize] {
            if c.dist>0.0 || c.exclude!=0 || c.geom.iter().any(|g| *g<0) { continue; }
            let ga=c.geom[0] as usize; let gb=c.geom[1] as usize;
            let pair=(ga.min(gb),ga.max(gb));
            self.next_touching.push(pair);
            let event=match (self.geom_parts[ga],self.geom_parts[gb]) {
                (Some(a),Some(b)) => Some((a.min(b),Some(a.max(b)))),
                (Some(a),None)|(None,Some(a)) => Some((a,None)),
                _ => None
            };
            let Some((a,b))=event else { continue };

            let va=self.velocity(self.model.geom_bodyid[ga] as usize,c.pos);
            let vb=self.velocity(self.model.geom_bodyid[gb] as usize,c.pos);
            let speed=dot(sub(va,vb),[c.frame[0],c.frame[1],c.frame[2]]);
            if self.touching.binary_search(&pair).is_err() { self.begins.push((a,b,speed.max(0.0))); }
            if speed>BOUNCE_SPEED { self.hit_events.push((a,b,speed)); }
        }
        self.next_touching.sort_unstable();
        self.next_touching.dedup();
        std::mem::swap(&mut self.touching,&mut self.next_touching);
    }
    pub fn contact_begins(&self) -> Vec<Event> { self.begins.clone() }
    pub fn hits(&self) -> Vec<Event> { self.hit_events.clone() }

    pub fn angles(&mut self, asm: &Assembly) -> Vec<f64> {
        let poses=self.poses();
        asm.joints.iter().enumerate().map(|(i,j)| {
            if matches!(j.kind,JointKind::Weld) { return 0.0; }
            if let Some(id)=self.joints[i] {
                if self.model.jnt_type[id]!=JNT_BALL { return self.data.qpos[self.model.jnt_qposadr[id] as usize]; }
            }
            let qa=quaternion(&self.data.xquat,self.bodies[j.a]);
            let qb=quaternion(&self.data.xquat,self.bodies[j.b]);
            match j.kind {
                JointKind::Spherical { .. }|JointKind::Flex {one_way:false,..} => {
                    dot(rotated(qa,norm(j.axis)),rotated(qb,norm(j.axis))).clamp(-1.0,1.0).acos()
                }
                JointKind::Prismatic { .. } => {
                    dot(sub(poses[j.b].apply(self.rest_com(j.b),j.pivot),poses[j.a].apply(self.rest_com(j.a),j.pivot)),rotated(qa,norm(j.axis)))
                }
                _ => {
                    let mut rel=[0.0;4];
                    util_spatial::mul_quat(&mut rel,&[qa[0],-qa[1],-qa[2],-qa[3]],&qb);
                    2.0*dot([rel[1],rel[2],rel[3]],norm(j.axis)).atan2(rel[0])
                }
            }
        }).collect()
    }

    // The motors' gains for this substep ([`SERVO_BAND_HINGE`] and the others).
    fn tune_motors(&mut self) {
        let fastest = MOTOR_SWING / self.model.opt.timestep;
        for m in &self.motors {
            let i = m.actuator;
            let inertia = 1.0 / self.model.actuator_acc0[i].max(MINVAL);
            let limit = self.model.actuator_forcerange[2*i+1];
            let slide = self.model.jnt_type[self.model.actuator_trnid[2*i] as usize] == JNT_SLIDE;
            let (gain, bias) = (&mut self.model.actuator_gainprm[NGAIN*i..NGAIN*(i+1)], &mut self.model.actuator_biasprm[NBIAS*i..NBIAS*(i+1)]);
            match m.kind {
                crate::MotorKind::Position => {
                    let band = if slide { SERVO_BAND_SLIDER } else { SERVO_BAND_HINGE };
                    let kp = (limit / band).min(inertia * fastest * fastest);
                    gain[0] = kp;
                    bias[1] = -kp;
                    bias[2] = -2.0 * (kp * inertia).sqrt();
                }
                crate::MotorKind::Speed => {
                    let band = if slide { DRIVE_BAND_SLIDER } else { DRIVE_BAND_HINGE };
                    let kv = (limit / band).min(2.0 * inertia * fastest);
                    gain[0] = kv;
                    bias[2] = -kv;
                }
            }
        }
    }

    /// Sets joint `j`'s motor: a servo's angle or travel to go to (B relative to A from where it printed, within
    /// the joint's range), a drive's speed. A joint without a motor, or a target that is not finite, is left as it is.
    pub fn set_motor(&mut self, j: usize, target: f64) {
        let Some(k) = self.motors.iter().position(|m| m.joint == j) else { return };
        let Some(id) = self.joints[j] else { return };
        if !target.is_finite() { return; }
        let target = if self.motors[k].kind == crate::MotorKind::Position && self.model.jnt_limited[id] != 0 {
            target.clamp(self.model.jnt_range[2*id], self.model.jnt_range[2*id+1])
        } else {
            target
        };
        self.motors[k].target = target;
        self.data.ctrl[self.model.actuator_ctrladr[self.motors[k].actuator] as usize] = target;
        self.wake(&[self.model.jnt_bodyid[id] as usize]);
    }

    /// Each motor's joint and what it is set to.
    pub fn motors(&self) -> Vec<(usize, f64)> { self.motors.iter().map(|m| (m.joint, m.target)).collect() }

    pub fn set_limits(&mut self, j: usize, kind: &JointKind) {
        // Only a weld has no joint.
        let Some(id)=self.joints[j] else { return };
        self.wake(&[self.model.jnt_bodyid[id] as usize]);
        let range=match *kind {
            JointKind::Revolute {lower,upper} => { self.model.jnt_limited[id]=u8::from(!kind.turns_freely()); [lower,upper] }
            JointKind::Prismatic {lower,upper} => [lower,upper],
            JointKind::Spherical {cone,..} => [0.0,cone.clamp(0.0,0.5*std::f64::consts::PI)],
            _ => return
        };
        self.model.jnt_range[2*id..2*id+2].copy_from_slice(&range);
        self.assembly.joints[j].kind=kind.clone();
        for body in &mut self.spec.bodies { for joint in &mut body.joints {
            if joint.name.as_deref()==Some(joint_name(j).as_str()) { joint.range=range; joint.limited=self.model.jnt_limited[id] as i32; }
        }}
    }
    pub fn joint_force(&self, j: usize) -> f64 {
        let Some(id)=self.joints[j] else { return 0.0 };
        len(triple(&self.data.cfrc_int,2*self.model.jnt_bodyid[id] as usize+1))
    }

    pub fn grab(&mut self, body: usize, point: P3, stiffness_hz: f64) {
        assert!(stiffness_hz>0.0 && stiffness_hz.is_finite(),"grab stiffness must be finite and positive");
        let id=self.bodies[body];
        let held=self.assembly.anchors.iter().any(|a| a.kind==AnchorKind::Held && self.bodies[a.body]==id);
        if held && self.model.body_rootid[id] as usize==self.ground {
            let m=self.model.body_mocapid[self.ground] as usize;
            let local=unrotated(quaternion(&self.data.mocap_quat,m),sub(point,triple(&self.data.mocap_pos,m)));
            self.release();
            self.carry=Some(Carry {body,local});
            return;
        }
        self.wake(&[id]);
        let local=self.local_point(id,point);
        let mass=self.assembly.bodies[body].mass.mass;
        let com=self.world_point(id,sub(self.rest_com(body),self.origins[body]));
        let lever=sub(point,com);
        let r=len(lever);
        let axis=if r>1e-9 {norm(unrotated(quaternion(&self.data.xquat,id),lever))} else {[0.0,0.0,1.0]};
        let inertia=self.assembly.bodies[body].mass.inertia;
        let across=0.5*((0..3).map(|k|inertia[k][k]).sum::<f64>()-inertia_about(inertia,axis));
        let swing=2.0*GRIP_SWING_DAMPING_RATIO*((across+mass*r*r)*mass*len(self.gravity)*r).sqrt();
        let effective=1.0/(1.0/mass+r*r/across.max(1e-9));
        let bob=2.0*GRAB_DAMPING_RATIO*std::f64::consts::TAU*stiffness_hz*(effective/mass).sqrt();
        let linear_damping=if r>1e-6 {bob.min(swing/(mass*r*r))} else {bob};
        let angular_damping=((swing-mass*r*r*linear_damping)/across.max(1e-9)).max(GRIP_SPIN_DAMPING);
        // 200 times the part's weight, and at least ten times everything's, as the current engine's hand
        let all: f64=self.assembly.bodies.iter().map(|b| b.mass.mass).sum();
        let max_force=(mass*200.0).max(all*10.0)*GRAVITY;
        self.release();
        self.grab=Some(Grab {body,local,from:point,at:point,hertz:stiffness_hz,max_force,velocity:[0.0;3],linear_damping,angular_damping,dry_torque:GRIP_STICTION*mass*len(self.gravity)*r});
    }
    // The hand is the current engine's grab spring: a soft constraint at `hertz`, critically damped on the
    // held point's effective mass as a free body (so a pull its joints hold loads them, as there), with its
    // pull capped at max_force, so a hand cannot pull a part through what holds it. One substep of that
    // constraint's soft step, applied as a force at the held point.
    fn pull(&mut self, target: P3, target_velocity: P3, h: f64) {
        let Some(g)=&self.grab else { return };
        let id=self.bodies[g.body];
        let point=self.world_point(id,g.local);
        let velocity=self.velocity(id,point);
        let lever=sub(point,triple(&self.data.xipos,id));
        let frame: [f64;9]=self.data.ximat[9*id..9*id+9].try_into().unwrap();
        let inertia=scale(triple(&self.model.body_inertia,id),1.0/self.shares[id]);
        let mass=self.model.body_mass[id]/self.shares[id];
        // The point's inverse mass, 1/m - [r] I^-1 [r], column by column.
        let turn=|x: P3| {
            let local: P3=std::array::from_fn(|k| (frame[k]*x[0]+frame[3+k]*x[1]+frame[6+k]*x[2])/inertia[k]);
            [dot([frame[0],frame[1],frame[2]],local),dot([frame[3],frame[4],frame[5]],local),dot([frame[6],frame[7],frame[8]],local)]
        };
        let columns: [P3;3]=std::array::from_fn(|c| {
            let e: P3=std::array::from_fn(|k| if k==c {1.0} else {0.0});
            sub(scale(e,1.0/mass),cross(lever,turn(cross(lever,e))))
        });
        let omega=std::f64::consts::TAU*g.hertz;
        let a1=2.0*GRAB_DAMPING_RATIO+h*omega;
        let a2=h*omega*a1;
        let bias_rate=omega/a1;
        let mass_scale=a2/(1.0+a2);
        let want=add(scale(sub(target,point),bias_rate),sub(target_velocity,velocity));
        // Solve columns * f = want (Cramer's rule on the symmetric 3x3).
        let det=dot(columns[0],cross(columns[1],columns[2]));
        let solved=[dot(want,cross(columns[1],columns[2]))/det,dot(columns[0],cross(want,columns[2]))/det,dot(columns[0],cross(columns[1],want))/det];
        let mut force=scale(solved,mass_scale/h);
        if len(force)>g.max_force { force=scale(force,g.max_force/len(force)); }
        let torque=cross(lever,force);
        self.data.xfrc_applied[6*id..6*id+3].copy_from_slice(&force);
        self.data.xfrc_applied[6*id+3..6*id+6].copy_from_slice(&torque);
    }
    pub fn dangle(&mut self, body: usize, point: P3) { self.grab(body,point,DANGLE_HZ); }
    pub fn drag(&mut self, point: P3, dt: f64) {
        if let Some(c)=&self.carry {
            let m=self.model.body_mocapid[self.ground] as usize;
            let q=quaternion(&self.data.mocap_quat,m);
            let at=sub(point,rotated(q,c.local));
            self.wake_all();
            self.set_ground(at,q,dt);
        }
        if let Some(g)=&mut self.grab {
            let delta=sub(point,g.at);
            let cap=self.assembly.max_speed*dt;
            let at=if cap>0.0 && len(delta)>cap { madd(g.at,delta,cap/len(delta)) } else { point };
            if dt>0.0 {
                let raw=scale(sub(at,g.at),1.0/dt);
                g.velocity=madd(g.velocity,sub(raw,g.velocity),dt/(dt+HAND_SMOOTHING));
            }
            g.at=at;
        }
    }
    pub fn release(&mut self) { self.grab=None; self.carry=None; self.data.xfrc_applied.fill(0.0); }
    pub fn grabbed(&self) -> Option<usize> { self.grab.as_ref().map(|g|g.body).or(self.carry.as_ref().map(|c|c.body)) }
    /// A grab carries a held part (and the ground with it).
    pub fn carrying(&self) -> bool { self.carry.is_some() }
    /// Where the ground (what holds the held and fixed parts) is now.
    pub fn ground_at(&self) -> P3 { triple(&self.data.mocap_pos,self.model.body_mocapid[self.ground] as usize) }
    /// Whether a part the ground does not hold (one that hangs from what it holds, or lies free) reaches below
    /// `height` against gravity.
    pub fn hangs_below(&self, height: f64) -> bool {
        let up=scale(norm(self.gravity),-1.0);
        (0..self.bodies.len()).filter(|&b| self.model.body_rootid[self.bodies[b]] as usize!=self.ground).any(|b| {
            let (lo,hi)=self.bounds(b);
            (0..8).any(|i| dot(std::array::from_fn(|k| if i&(1<<k)==0 {lo[k]} else {hi[k]}),up)<height)
        })
    }

    pub fn poke(&mut self, body: usize, point: P3, dv: P3) {
        self.wake(&[self.bodies[body]]);
        forward::fwd_position(&self.model,&mut self.data);
        let mut impulse=vec![0.0;self.model.nv];
        support::apply_ft(&self.model,&mut self.data,Some(&scale(dv,self.assembly.bodies[body].mass.mass)),None,&point,self.bodies[body] as i32,&mut impulse);
        core_smooth::solve_m(&self.model,&mut self.data,&mut impulse,None,1);
        for (v,dv) in self.data.qvel.iter_mut().zip(impulse) { *v+=dv; }
        core_smooth::com_vel(&self.model,&mut self.data);
    }
    // A constrained tree cannot receive an arbitrary Cartesian velocity without projection.
    pub fn throw(&mut self, velocity: P3) {
        let Some((body,local))=self.grab.as_ref().map(|g| (g.body,g.local)) else { return };
        let id=self.bodies[body];
        self.wake(&[id]);
        let root=self.model.body_rootid[id] as usize;
        let joint=self.model.body_jntadr[root];
        if joint<0 || self.model.jnt_type[joint as usize]!=JNT_FREE {
            return;
        }
        let dv=sub(velocity,self.velocity(id,self.world_point(id,local)));
        let adr=self.model.jnt_dofadr[joint as usize] as usize;
        for (v,d) in self.data.qvel[adr..adr+3].iter_mut().zip(dv) { *v+=d; }
        core_smooth::com_vel(&self.model,&mut self.data);
    }

    pub fn pin(&mut self, body: usize, point: P3) {
        if let Some(i)=self.pins.iter().position(|p|p.body==body) { self.pins.remove(i); }
        else { self.pins.push(Pin {body,local:self.local_point(self.bodies[body],point),world:point}); }
        self.rebuild();
    }
    pub fn pinned(&self) -> Vec<usize> { self.pins.iter().map(|p|p.body).collect() }
    pub fn add_ball(&mut self, centre: P3, radius: f64) {
        self.spec.bodies[0].geoms.push(GeomSpec {pos:centre,size:[radius,0.0,0.0],..geom(&self.assembly,self.model.opt.timestep,1)});
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let mut compiled=self.spec.clone();
        for p in &self.pins {
            let mut eq=connect(self.part_name(p.body),None,p.local,self.model.opt.timestep);
            eq.name=Some(format!("pin_{}",p.body));
            compiled.equalities.push(eq);
        }
        for f in &self.frozen {
            let mut eq=connect(self.part_name(f.body),None,[0.0;3],self.model.opt.timestep);
            eq.name=Some(format!("frozen_{}",f.body));
            eq.r#type=EQ_WELD;
            eq.data[10]=1.0;
            compiled.equalities.push(eq);
        }
        let mut model=builder::compile(&compiled).expect("valid mechanism topology must compile");
        attach_fields(&mut model,&self.fields);
        for p in &self.pins {
            let id=name::name2id(&model,OBJ_EQUALITY,&format!("pin_{}",p.body)) as usize;
            model.eq_data[id*NEQDATA+3..id*NEQDATA+6].copy_from_slice(&p.world);
        }
        for f in &self.frozen {
            let id=name::name2id(&model,OBJ_EQUALITY,&format!("frozen_{}",f.body)) as usize;
            let d=&mut model.eq_data[id*NEQDATA..(id+1)*NEQDATA];
            // The world anchor is data[0..3]; the body's anchor is data[3..6].
            d[..3].copy_from_slice(&f.pos);
            d[3..6].fill(0.0);
            d[6..10].copy_from_slice(&[f.quat[0],-f.quat[1],-f.quat[2],-f.quat[3]]);
        }
        let mut data=io::make_data(&model);
        for j in 0..model.njnt {
            let label=String::from_utf8_lossy(name::id2name(&model,OBJ_JOINT,j as i32).unwrap());
            let old=name::name2id(&self.model,OBJ_JOINT,&label);
            let q=model.jnt_qposadr[j] as usize;
            let v=model.jnt_dofadr[j] as usize;
            let (nq,nv)=joint_size(model.jnt_type[j]);
            if old>=0 {
                let old=old as usize;
                let oq=self.model.jnt_qposadr[old] as usize;
                let ov=self.model.jnt_dofadr[old] as usize;
                data.qpos[q..q+nq].copy_from_slice(&self.data.qpos[oq..oq+nq]);
                data.qvel[v..v+nv].copy_from_slice(&self.data.qvel[ov..ov+nv]);
                data.qacc_warmstart[v..v+nv].copy_from_slice(&self.data.qacc_warmstart[ov..ov+nv]);
            } else if model.jnt_type[j]==JNT_FREE {
                let body=model.jnt_bodyid[j] as usize;
                let label=String::from_utf8_lossy(name::id2name(&model,OBJ_BODY,body as i32).unwrap());
                let old=name::name2id(&self.model,OBJ_BODY,&label) as usize;
                let pos=triple(&self.data.xpos,old);
                let quat=quaternion(&self.data.xquat,old);
                let mut vel=[0.0;6];
                core_util::object_velocity(&self.model,&self.data,OBJ_XBODY,old,&mut vel,0);
                data.qpos[q..q+3].copy_from_slice(&pos);
                data.qpos[q+3..q+7].copy_from_slice(&quat);
                let mut lin: P3=vel[3..].try_into().unwrap();
                let mut ang: P3=vel[..3].try_into().unwrap();
                if self.model.body_rootid[old]==self.ground as i32 {
                    // Let go of the moving ground, the body keeps the ground's velocity at its point.
                    let (v_at,spin)=self.ground_motion(pos);
                    lin=add(lin,v_at);
                    ang=add(ang,spin);
                }
                data.qvel[v..v+3].copy_from_slice(&lin);
                data.qvel[v+3..v+6].copy_from_slice(&unrotated(quat,ang));
            }
        }
        data.act.copy_from_slice(&self.data.act);
        for b in 1..model.nbody {
            let mocap=model.body_mocapid[b];
            if mocap<0 { continue; }
            let label=String::from_utf8_lossy(name::id2name(&model,OBJ_BODY,b as i32).unwrap());
            let old=name::name2id(&self.model,OBJ_BODY,&label) as usize;
            let om=self.model.body_mocapid[old] as usize;
            let m=mocap as usize;
            data.mocap_pos[3*m..3*m+3].copy_from_slice(&self.data.mocap_pos[3*om..3*om+3]);
            data.mocap_quat[4*m..4*m+4].copy_from_slice(&self.data.mocap_quat[4*om..4*om+4]);
        }
        data.time=self.data.time;
        for b in &mut self.bodies {
            let label=String::from_utf8_lossy(name::id2name(&self.model,OBJ_BODY,*b as i32).unwrap());
            *b=name::name2id(&model,OBJ_BODY,&label) as usize;
        }
        for (i,j) in self.joints.iter_mut().enumerate() {
            let id=name::name2id(&model,OBJ_JOINT,&joint_name(i));
            *j=(id>=0).then_some(id as usize);
        }
        self.anchor_joints=self.assembly.anchors.iter().enumerate().filter_map(|(i,a)| {
            let id=name::name2id(&model,OBJ_JOINT,&format!("anchor_{i}"));
            (id>=0).then_some((id as usize,a.friction))
        }).collect();
        data.ctrl.copy_from_slice(&self.data.ctrl);
        data.threadpool=self.data.threadpool.clone();
        self.model=model; self.data=data;
        self.configure();
        self.tune_motors();
        self.touching.clear(); self.next_touching.clear();
        forward::forward(&self.model,&mut self.data);
    }

    fn part_name(&self, body: usize) -> String {
        String::from_utf8(name::id2name(&self.model,OBJ_BODY,self.bodies[body] as i32).unwrap().to_vec()).unwrap()
    }

    // The hand's damping over the step (the held part drawn to the hand's velocity, its spin slowed), as the
    // current engine's grip has it: an implicit step's impulse, applied as a force over the step's first substep
    // (`h`), so the contact solve meets it with the hand's pull. Added to the velocities ahead of the solve, it
    // drove a part held against a wall into it, the solve's rebound lifted the part off its contacts, and the
    // pull threw it in for a substep: up to 0.9 mm into the wall, every few steps of a hard drag.
    fn damp_grip(&mut self, dt:f64, h:f64) {
        let Some(g)=&self.grab else {return;};
        let body=self.bodies[g.body];
        let mut velocity=[0.0;6];
        core_util::object_velocity(&self.model,&self.data,OBJ_BODY,body,&mut velocity,0);
        let q=quaternion(&self.data.xquat,body);
        let spin=unrotated(q,velocity[..3].try_into().unwrap());
        let inertia=self.assembly.bodies[g.body].mass.inertia;
        let momentum: P3=std::array::from_fn(|i|(0..3).map(|k|inertia[i][k]*spin[k]).sum());
        let keep=(1.0/(1.0+dt*g.angular_damping)-dt*g.dry_torque/len(momentum).max(1e-9)).max(0.0);
        let torque=rotated(q,scale(momentum,keep-1.0));
        let mass=self.assembly.bodies[g.body].mass.mass;
        let force=scale(sub(g.velocity,velocity[3..].try_into().unwrap()),mass*dt*g.linear_damping/(1.0+dt*g.linear_damping));
        for k in 0..3 {
            self.data.xfrc_applied[6*body+k]+=force[k]/h;
            self.data.xfrc_applied[6*body+3+k]+=torque[k]/h;
        }
    }

    // The engine meets an equality at the acceleration level, so a loop moving fast drifts open by about
    // its velocity squared times the weld's time constant squared (0.02 mm on a spatial loop turning at 30
    // rad/s). After each substep its closures are projected back, positions (two Newton steps) then
    // velocities, in the mass metric: what a rigid closure would keep, the least change to the motion.
    fn close_loops(&mut self) {
        if self.loops.is_empty() { return; }
        let loops: Vec<usize> = self.loops.iter().copied()
            .filter(|&e| sleep::sleep_state(&self.model,&self.data,OBJ_EQUALITY,e as i32) != S_ASLEEP).collect();
        if loops.is_empty() { return; }
        let nv = self.model.nv;
        let rows = 6*loops.len();
        let mut jac = vec![0.0; rows*nv];
        let mut error = vec![0.0; rows];
        let mut change = vec![0.0; nv];
        for pass in 0..3 {
            core_smooth::kinematics(&self.model,&mut self.data);
            core_smooth::com_pos(&self.model,&mut self.data);
            self.loop_rows(&loops,&mut jac,&mut error);
            if pass == 2 {
                for (r,e) in error.iter_mut().enumerate() {
                    *e = jac[r*nv..(r+1)*nv].iter().zip(&self.data.qvel).map(|(j,v)| j*v).sum();
                }
            }
            self.least_change(&jac,&error,&mut change);
            if pass < 2 { support::integrate_pos(&self.model,&mut self.data.qpos,&change,1.0); }
            else { for (v,c) in self.data.qvel.iter_mut().zip(&change) { *v += c; } }
        }
        core_smooth::com_vel(&self.model,&mut self.data);
    }

    // Per loop weld, six rows: the gap between its anchors and the rotation between its bodies (world
    // frame), with their Jacobians.
    fn loop_rows(&self, loops: &[usize], jac: &mut [f64], error: &mut [f64]) {
        let (m,d) = (&self.model,&self.data);
        let nv = m.nv;
        let (mut jp1,mut jr1,mut jp2,mut jr2) = (vec![0.0;3*nv],vec![0.0;3*nv],vec![0.0;3*nv],vec![0.0;3*nv]);
        for (k,&e) in loops.iter().enumerate() {
            let (b1,b2) = (m.eq_obj1id[e] as usize,m.eq_obj2id[e] as usize);
            let data = &m.eq_data[NEQDATA*e..NEQDATA*(e+1)];
            let at = |b: usize,local: &[f64]| {
                let x = &d.xmat[9*b..9*b+9];
                add(triple(&d.xpos,b),std::array::from_fn(|r| x[3*r]*local[0]+x[3*r+1]*local[1]+x[3*r+2]*local[2]))
            };
            let (p1,p2) = (at(b1,&data[3..6]),at(b2,&data[..3]));
            core_util::jac(m,d,Some(&mut jp1),Some(&mut jr1),&p1,b1);
            core_util::jac(m,d,Some(&mut jp2),Some(&mut jr2),&p2,b2);
            let (mut q1,mut neg2,mut dif,mut turn) = ([0.0;4],[0.0;4],[0.0;4],[0.0;3]);
            util_spatial::mul_quat(&mut q1,&quaternion(&d.xquat,b1),data[6..10].try_into().unwrap());
            util_spatial::neg_quat(&mut neg2,&quaternion(&d.xquat,b2));
            util_spatial::mul_quat(&mut dif,&q1,&neg2);
            util_spatial::quat2vel(&mut turn,&dif,1.0);
            let gap = sub(p1,p2);
            for r in 0..3 {
                error[6*k+r] = gap[r];
                error[6*k+3+r] = turn[r];
                for v in 0..nv {
                    jac[(6*k+r)*nv+v] = jp1[r*nv+v]-jp2[r*nv+v];
                    jac[(6*k+3+r)*nv+v] = jr1[r*nv+v]-jr2[r*nv+v];
                }
            }
        }
    }

    // The joint change of least kinetic metric that cancels the rows' error: -M^-1 J^T (J M^-1 J^T)^-1 e.
    // A loop's rows can depend on each other (a planar loop cannot move out of its plane): a trace of
    // damping, 1e-12 of the largest diagonal, keeps those rows from amplifying round-off.
    fn least_change(&mut self, jac: &[f64], error: &[f64], change: &mut [f64]) {
        let nv = self.model.nv;
        let rows = error.len();
        let mut inv = jac.to_vec();
        core_smooth::solve_m(&self.model,&mut self.data,&mut inv,None,rows);
        let mut a = vec![0.0; rows*rows];
        for r in 0..rows { for c in 0..rows {
            a[r*rows+c] = jac[r*nv..(r+1)*nv].iter().zip(&inv[c*nv..(c+1)*nv]).map(|(x,y)| x*y).sum();
        } }
        let damping = 1e-12*(0..rows).map(|r| a[r*rows+r]).fold(f64::MIN_POSITIVE,f64::max);
        for r in 0..rows { a[r*rows+r] += damping; }
        util_solve::chol_factor(&mut a,rows,damping);
        let mut lambda = vec![0.0; rows];
        util_solve::chol_solve(&mut lambda,&a,error,rows);
        change.fill(0.0);
        for (r,l) in lambda.iter().enumerate() {
            for (c,y) in change.iter_mut().zip(&inv[r*nv..(r+1)*nv]) { *c -= l*y; }
        }
    }

    // The engine may put a tree to sleep only once it has been under the sleep speed (its own measure: each
    // dof's speed times its length) for SLEEP_TIME.
    fn gate_sleep(&mut self, h: f64) {
        if self.model.opt.enableflags & ENBL_SLEEP == 0 { return; }
        let (m,d) = (&mut self.model,&self.data);
        for t in 0..m.ntree {
            if d.tree_asleep[t] >= 0 { continue; }
            let adr = m.tree_dofadr[t] as usize;
            let slow = (adr..adr+m.tree_dofnum[t] as usize).all(|v| m.dof_length[v]*d.qvel[v].abs() < m.opt.sleep_tolerance);
            self.still[t] = if slow { self.still[t]+h } else { 0.0 };
            m.tree_sleep_policy[t] = if self.still[t] >= SLEEP_TIME { SLEEP_ALLOWED } else { SLEEP_NEVER };
        }
    }

    /// Wakes every part, as a hand that moves the ground does.
    pub fn wake_all(&mut self) {
        let everything=self.bodies.clone();
        self.wake(&everything);
    }

    // Wakes the trees of these model bodies with their islands now, each for SLEEP_TIME at least: a hand on a
    // part wakes it at once and keeps it awake as long as a part just made. (The engine itself wakes a tree
    // that a force, a contact or a constraint reaches, or whose pose or velocity was changed, as when the
    // ground moves, at its next step.)
    fn wake(&mut self, bodies: &[usize]) {
        if self.model.opt.enableflags & ENBL_SLEEP == 0 { return; }
        let ntree = self.model.ntree;
        let asleep: Vec<bool> = self.data.tree_asleep.iter().map(|&a| a >= 0).collect();
        for &b in bodies {
            let tree = self.model.body_treeid[b];
            if tree < 0 { continue; }
            if asleep[tree as usize] { sleep::wake_island(&mut self.data.tree_asleep,ntree,tree,AWAKE,None,self.data.time); }
            self.still[tree as usize] = 0.0;
            self.model.tree_sleep_policy[tree as usize] = SLEEP_NEVER;
        }
        let mut woke = false;
        for t in 0..ntree {
            if asleep[t] && self.data.tree_asleep[t] < 0 {
                woke = true;
                self.still[t] = 0.0;
                self.model.tree_sleep_policy[t] = SLEEP_NEVER;
            }
        }
        if woke { sleep::update_sleep(&self.model,&mut self.data); }
    }

    /// The parts asleep (resting, not simulated until something wakes them).
    pub fn sleeping(&self) -> Vec<usize> {
        (0..self.bodies.len()).filter(|&i| self.data.body_awake[self.bodies[i]] == S_ASLEEP).collect()
    }

    fn limit_speed(&mut self) {
        let cap=self.assembly.max_speed;
        if cap<=0.0 { return; }
        self.speed_scale.fill(1.0);
        for (i,&body) in self.bodies.iter().enumerate() {
            let speed=len(self.velocity(body,self.world_point(body,sub(self.rest_com(i),self.origins[i]))));
            if speed>cap {
                let root=self.model.body_rootid[body] as usize;
                self.speed_scale[root]=self.speed_scale[root].min(cap/speed);
            }
        }
        // Scale each articulated tree together to retain its velocity constraints.
        for v in 0..self.model.nv {
            let root=self.model.body_rootid[self.model.dof_bodyid[v] as usize] as usize;
            self.data.qvel[v]*=self.speed_scale[root];
        }
    }

    pub fn move_ground(&mut self, at: P3, axis: P3, ang: f64, dt: f64) {
        let mut q=[0.0;4];
        util_spatial::axis_angle2quat(&mut q,&norm(axis),ang);
        self.set_ground(at,q,dt);
    }

    pub fn carry_ground(&mut self, hand: (P3,P3,f64), shake: (P3,P3,f64), ease: f64, dt: f64) -> bool {
        let mut hq=[0.0;4]; let mut sq=[0.0;4]; let mut q=[0.0;4];
        util_spatial::axis_angle2quat(&mut hq,&norm(hand.1),hand.2);
        util_spatial::axis_angle2quat(&mut sq,&norm(shake.1),shake.2);
        util_spatial::mul_quat(&mut q,&hq,&sq);
        let at=add(hand.0,rotated(hq,shake.0));
        let left=self.held_move(at,q);
        let time=if left<0.01 {dt} else if self.assembly.max_speed>0.0 {ease.max(left/self.assembly.max_speed)} else {ease};
        self.set_ground(at,q,time);
        left>=0.01
    }

    fn held_move(&self, at:P3, q:[f64;4]) -> f64 {
        let m=self.model.body_mocapid[self.ground] as usize;
        let now=triple(&self.data.mocap_pos,m);
        let nq=quaternion(&self.data.mocap_quat,m);
        let mut turn=[0.0;3]; util_spatial::sub_quat(&mut turn,&q,&nq);
        self.assembly.anchors.iter().filter(|a|self.model.body_rootid[self.bodies[a.body]] as usize==self.ground).map(|a| {
            len(sub(add(at,rotated(q,self.rest_com(a.body))),add(now,rotated(nq,self.rest_com(a.body)))))+len(turn)*reach(&self.assembly,a.body)
        }).fold(0.0,f64::max)
    }

    fn set_ground(&mut self, at:P3, q:[f64;4], duration:f64) {
        let m=self.model.body_mocapid[self.ground] as usize;
        let duration=duration.max(self.model.opt.timestep);
        let duration=if self.assembly.max_speed>0.0 {duration.max(self.held_move(at,q)/self.assembly.max_speed)} else {duration};
        self.ground_velocity=scale(sub(at,triple(&self.data.mocap_pos,m)),1.0/duration);
        util_spatial::sub_quat(&mut self.ground_spin,&q,&quaternion(&self.data.mocap_quat,m));
        self.ground_spin=scale(self.ground_spin,1.0/duration);
        self.ground_left=duration;
    }

    fn advance_ground(&mut self, dt:f64) {
        let dt=dt.min(self.ground_left);
        if dt<=0.0 {return;}
        let m=self.model.body_mocapid[self.ground] as usize;
        let p=madd(triple(&self.data.mocap_pos,m),self.ground_velocity,dt);
        let mut q=quaternion(&self.data.mocap_quat,m);
        util_spatial::quat_integrate(&mut q,&self.ground_spin,dt);
        self.data.mocap_pos[3*m..3*m+3].copy_from_slice(&p);
        self.data.mocap_quat[4*m..4*m+4].copy_from_slice(&q);
        self.ground_left-=dt;
    }

    fn let_go(&mut self, body:usize) -> bool {
        let mut held=false;
        if self.grabbed()==Some(body) { self.release(); held=true; }
        self.pins.retain(|p| { if p.body==body {held=true;false} else {true} });
        let label=self.part_name(body);
        let b=self.spec.bodies.iter_mut().find(|b|b.name.as_deref()==Some(&label)).unwrap();
        if b.parent==1 {
            b.parent=0;
            b.joints.clear();
            b.joints.push(JointSpec {name:Some(format!("free_{}",label.trim_start_matches("part_"))),r#type:JNT_FREE,limited:0,..JointSpec::default()});
            held=true;
        }
        held
    }

    pub fn is_frozen(&self, body:usize) -> bool {self.frozen.iter().any(|f|f.body==body)}
    pub fn frozen(&self) -> Vec<usize> {self.frozen.iter().map(|f|f.body).collect()}

    fn freeze_one(&mut self, body:usize, on:bool) {
        if on && !self.is_frozen(body) {
            let id=self.bodies[body];
            self.frozen.push(Frozen {body,pos:triple(&self.data.xpos,id),quat:quaternion(&self.data.xquat,id)});
            self.stand_still(body);
        } else if !on {self.frozen.retain(|f|f.body!=body);}
    }

    fn stand_still(&mut self, body:usize) {
        let id=self.bodies[body];
        let adr=self.model.body_dofadr[id];
        let n=self.model.body_dofnum[id] as usize;
        if adr>=0 {self.data.qvel[adr as usize..adr as usize+n].fill(0.0);self.data.qacc_warmstart[adr as usize..adr as usize+n].fill(0.0);}
    }

    fn translate_set(&mut self, bodies:&[usize], delta:P3) {
        let mut roots=Vec::new();
        for &body in bodies {
            let root=self.model.body_rootid[self.bodies[body]] as usize;
            assert!(self.bodies.iter().enumerate().all(|(i,&b)|self.model.body_rootid[b] as usize!=root || bodies.contains(&i)),
                "moving part of an articulated tree needs Cartesian pose projection");
            if roots.contains(&root) {continue;}
            let j=self.model.body_jntadr[root];
            assert!(j>=0 && self.model.jnt_type[j as usize]==JNT_FREE,
                "moving an anchored tree needs its anchors released first");
            let q=self.model.jnt_qposadr[j as usize] as usize;
            for (p,d) in self.data.qpos[q..q+3].iter_mut().zip(delta) {*p+=d;}
            roots.push(root);
        }
        for &body in bodies {self.stand_still(body);}
    }

    // The bodies of the set that a lift can move: free (not held), and the whole of their articulated tree is in the
    // set. A body of a larger tree cannot be moved alone without projecting a Cartesian pose onto the joints (the
    // current engine moves it and its joints pull it back), so it stays where it is.
    fn liftable(&self, bodies:&[usize], held:&[bool]) -> Vec<usize> {
        let root=|b:usize| self.model.body_rootid[self.bodies[b]] as usize;
        bodies.iter().copied().zip(held).filter(|&(b,&h)| {
            let r=root(b);
            let j=self.model.body_jntadr[r];
            !h && j>=0 && self.model.jnt_type[j as usize]==JNT_FREE
                && (0..self.bodies.len()).filter(|&o|root(o)==r).all(|o|bodies.iter().zip(held).any(|(&c,&hc)|c==o && !hc))
        }).map(|(b,_)|b).collect()
    }

    // The ground's velocity and spin (world frame) while it is moving, and its velocity at `point`.
    fn ground_motion(&self, point:P3) -> (P3,P3) {
        if self.ground_left<=1e-9 { return ([0.0;3],[0.0;3]); }
        let m=self.model.body_mocapid[self.ground] as usize;
        let spin=rotated(quaternion(&self.data.mocap_quat,m),self.ground_spin);
        (add(self.ground_velocity,cross(spin,sub(point,triple(&self.data.mocap_pos,m)))),spin)
    }

    fn bounds(&self, body:usize) -> (P3,P3) {
        let b=&self.assembly.bodies[body];
        let id=self.bodies[body];
        let mut lo=[f64::INFINITY;3];let mut hi=[f64::NEG_INFINITY;3];
        let mut include=|p:P3,r:f64| {let p=self.world_point(id,sub(p,self.origins[body]));for k in 0..3 {lo[k]=lo[k].min(p[k]-r);hi[k]=hi[k].max(p[k]+r);}};
        for (pts,r) in &b.tubes {for &p in pts {include(p,*r);}}
        for &(a,b,r) in &b.rods {include(a,r);include(b,r);}
        for &p in b.hulls.iter().flatten() {include(p,0.0);}
        (lo,hi)
    }

    pub fn freeze(&mut self, bodies:&[usize], on:bool) {self.apply(bodies,&BodyAction::Freeze(on));}

    // Every action rebuilds the model, which wakes every tree.
    pub fn apply(&mut self, bodies:&[usize], action:&BodyAction) {
        let bodies:Vec<_>=bodies.iter().copied().filter(|&b|b<self.bodies.len()).collect();
        if bodies.is_empty() {return;}
        let mut held=Vec::with_capacity(bodies.len());
        for &b in &bodies {
            match action {
                BodyAction::Freeze(on)=>self.freeze_one(b,*on),
                BodyAction::Pin=>{},
                _=>{
                    if !matches!(action,BodyAction::Unpin) {self.freeze_one(b,false);}
                    held.push(self.let_go(b));
                }
            }
        }
        self.rebuild();
        match action {
            BodyAction::Drop {lift}=>{
                if *lift>0.0 {
                    let free=self.liftable(&bodies,&held);
                    self.translate_set(&free,scale(norm(self.gravity),-*lift));
                }
            }
            BodyAction::Hang=>{
                let up=scale(norm(self.gravity),-1.0);
                let mut lo=[f64::INFINITY;3];let mut hi=[f64::NEG_INFINITY;3];
                for &b in &bodies {let (l,h)=self.bounds(b);for k in 0..3 {lo[k]=lo[k].min(l[k]);hi[k]=hi[k].max(h[k]);}}
                let delta=scale(up,len(sub(hi,lo)));
                // Hang lifts everything it let go of, held or not.
                let free=self.liftable(&bodies,&vec![false;bodies.len()]);
                self.translate_set(&free,delta);
                core_smooth::kinematics(&self.model,&mut self.data);
                let top=|b| {let (lo,hi)=self.bounds(b);(0..8).map(|i|dot(std::array::from_fn(|k|if i&(1<<k)==0 {lo[k]} else {hi[k]}),up)).fold(f64::NEG_INFINITY,f64::max)};
                let b=*bodies.iter().max_by(|&&a,&&b|top(a).total_cmp(&top(b))).unwrap();
                let com=self.world_point(self.bodies[b],sub(self.rest_com(b),self.origins[b]));
                let (lo,hi)=self.bounds(b);
                let at=madd(madd(com,up,(top(b)-dot(com,up)).max(0.0)),basis(up).0,0.125*len(sub(hi,lo)));
                self.pin(b,at);
            }
            BodyAction::Pin=>{
                for &b in &bodies {if !self.pins.iter().any(|p|p.body==b) {
                    let at=self.world_point(self.bodies[b],sub(self.rest_com(b),self.origins[b]));
                    self.pins.push(Pin {body:b,local:self.local_point(self.bodies[b],at),world:at});
                }}
                self.rebuild();
            }
            BodyAction::Throw(dv)=>{
                self.impulse.fill(0.0);
                for &b in &bodies {
                    let id=self.bodies[b];
                    let point=self.world_point(id,sub(self.rest_com(b),self.origins[b]));
                    let impulse=scale(*dv,self.assembly.bodies[b].mass.mass);
                    support::apply_ft(&self.model,&mut self.data,Some(&impulse),None,&point,id as i32,&mut self.impulse);
                }
                core_smooth::solve_m(&self.model,&mut self.data,&mut self.impulse,None,1);
                for (v,dv) in self.data.qvel.iter_mut().zip(&self.impulse) {*v+=dv;}
            }
            BodyAction::Home=>{
                for &b in &bodies {
                    let id=self.bodies[b];
                    let adr=self.model.body_jntadr[id];
                    if adr>=0 {for j in adr as usize..adr as usize+self.model.body_jntnum[id] as usize {
                        let q=self.model.jnt_qposadr[j] as usize;let (n,_)=joint_size(self.model.jnt_type[j]);
                        self.data.qpos[q..q+n].copy_from_slice(&self.model.qpos0[q..q+n]);
                    }}
                    self.stand_still(b);
                }
            }
            _=>{}
        }
        forward::forward(&self.model,&mut self.data);
    }
}

// A string's point may lie this far outside its part's solid (mm): on its surface, as a hole's centre in a
// thin wall is.
const STRING_POINT_SLACK: f64 = 0.5;

/// Checks the strings as [`DynWorld::new`] does before it builds: each point inside its part, and no string
/// shorter than its path as printed.
pub fn check_strings(asm: &Assembly) -> Result<(), StringFault> {
    let names: Vec<&str> = asm.bodies.iter().map(|b| b.name.as_str()).collect();
    check_string_parts(&asm.strings,&names,&|part,at| inside_part(&asm.bodies[part],at))
}

/// [`check_strings`] on parts known by their names and `inside(part, point)`: a host checks against each part's
/// [`Outline`] before it has the bodies.
pub fn check_string_parts(strings: &[StringSpec], names: &[&str], inside: &dyn Fn(usize, P3) -> bool) -> Result<(), StringFault> {
    for (s,string) in strings.iter().enumerate() {
        let points: Vec<_> = string.points().collect();
        for (k,&(part,at)) in points.iter().enumerate() {
            let Some(name) = names.get(part) else { return Err(StringFault::NoPart { string: s, part }) };
            if !inside(part,at) { return Err(StringFault::Outside { string: s, point: k, points: points.len(), part: name.to_string() }); }
        }
        let path: f64 = points.windows(2).map(|w| len(sub(w[1].1,w[0].1))).sum();
        if !(string.length > 0.0 && string.length.is_finite()) || path > string.length {
            return Err(StringFault::TooShort { string: s, path, length: string.length });
        }
    }
    Ok(())
}

// Inside the part: within the outline of all its hulls together (a hole's centre is a void, and a part's convex
// pieces may leave it out), or a tube's or rod's capsule, or a ball.
fn inside_part(b: &crate::sim::BodySpec, p: P3) -> bool {
    let capsule = |a: P3, c: P3, r: f64| {
        let d = sub(c,a);
        let t = (dot(sub(p,a),d)/dot(d,d).max(f64::MIN_POSITIVE)).clamp(0.0,1.0);
        len(sub(p,madd(a,d,t))) <= r+STRING_POINT_SLACK
    };
    let outline: Vec<P3> = b.hulls.iter().flatten().copied().collect();
    (!outline.is_empty() && Outline::new(&outline).contains(p))
        || b.tubes.iter().any(|(pts,r)| (0..pts.len()).any(|k| capsule(pts[k],pts[(k+1)%pts.len()],*r)))
        || b.rods.iter().any(|&(a,c,r)| capsule(a,c,r))
        || b.balls.iter().any(|&(c,r)| len(sub(p,c)) <= r+STRING_POINT_SLACK)
}

/// A part's outline for the strings' check: the convex hull of its points (its hulls' corners, or a mesh's
/// vertices), so a hole through the part has its centre inside. The hull is never built: a point's distance
/// to it comes from the points' extremes alone (GJK), so a meshed field's flat faces, a hair off flat, are no
/// trouble.
pub struct Outline {
    points: Vec<P3>,
}

impl Outline {
    pub fn new(points: &[P3]) -> Outline {
        Outline { points: points.to_vec() }
    }

    /// Within the outline, or [`STRING_POINT_SLACK`] outside it.
    pub fn contains(&self, p: P3) -> bool {
        let Some(&first) = self.points.first() else { return false };
        // from p, the outline's point farthest along d
        let support = |d: P3| self.points.iter().map(|q| sub(*q,p)).max_by(|a,b| dot(*a,d).total_cmp(&dot(*b,d))).unwrap();
        let mut simplex = vec![sub(first,p)];
        let mut v = simplex[0];
        loop {
            let d = len(v);
            if d <= STRING_POINT_SLACK { return true; }
            let w = support(scale(v,-1.0));
            // the whole outline lies beyond the plane through w across v: at least this far from p
            if dot(w,v)/d > STRING_POINT_SLACK { return false; }
            simplex.push(w);
            let (nearer, kept) = nearest_on_simplex(&simplex);
            // rounding stops the descent: p is as far as v says
            if dot(nearer,nearer) >= d*d { return false; }
            (v, simplex) = (nearer, kept);
        }
    }
}

// The point of the simplex `s` (at most four points) nearest the origin, and the fewest of its points that span
// it: over every subset, the origin's projection onto the subset's span, where that falls within the subset.
fn nearest_on_simplex(s: &[P3]) -> (P3, Vec<P3>) {
    let mut best: Option<(f64, P3, Vec<P3>)> = None;
    for mask in 1u32..(1 << s.len()) {
        let pts: Vec<P3> = (0..s.len()).filter(|i| mask >> i & 1 == 1).map(|i| s[i]).collect();
        let o = pts[0];
        let e: Vec<P3> = pts[1..].iter().map(|q| sub(*q,o)).collect();
        let mu: Vec<f64> = match e.len() {
            0 => Vec::new(),
            1 => {
                let g = dot(e[0],e[0]);
                if g <= 0.0 { continue; }
                vec![-dot(e[0],o)/g]
            }
            2 => {
                let (a,b,c) = (dot(e[0],e[0]),dot(e[0],e[1]),dot(e[1],e[1]));
                let det = a*c-b*b;
                if det <= 1e-12*a*c { continue; }
                let (r0,r1) = (-dot(e[0],o),-dot(e[1],o));
                vec![(r0*c-r1*b)/det,(a*r1-b*r0)/det]
            }
            _ => {
                let det = dot(e[0],cross(e[1],e[2]));
                if det.abs() <= 1e-12*len(e[0])*len(e[1])*len(e[2]) { continue; }
                let t = scale(o,-1.0);
                vec![dot(t,cross(e[1],e[2]))/det,dot(e[0],cross(t,e[2]))/det,dot(e[0],cross(e[1],t))/det]
            }
        };
        if mu.iter().any(|m| *m < 0.0) || mu.iter().sum::<f64>() > 1.0 { continue; }
        let x = e.iter().zip(&mu).fold(o,|x,(ei,m)| madd(x,*ei,*m));
        let d = dot(x,x);
        if best.as_ref().is_none_or(|b| d < b.0 || (d == b.0 && pts.len() < b.2.len())) { best = Some((d,x,pts)); }
    }
    // a single point is always its own nearest
    let (_,x,pts) = best.unwrap();
    (x,pts)
}

// A hull's points as the dynamics engine's mesh takes them. The engine refuses a flat, straight or
// single-point cloud (as its reference does); the current engine gives such a cloud a thickness of 1%
// of its size, at least 0.01 mm. Here that thickness is added along each thin axis of the cloud's
// own principal frame, so a tilted plate stays a plate.
fn solid_hull(points: &[P3]) -> Vec<P3> {
    let centre = scale(points.iter().fold([0.0;3],|a,p| add(a,*p)),1.0/points.len().max(1) as f64);
    let mut spread = [0.0;9];
    for p in points { let d = sub(*p,centre); for r in 0..3 { for c in 0..3 { spread[3*r+c] += d[r]*d[c]; } } }
    let mut value = [0.0;3]; let mut vector = [0.0;9]; let mut quat = [0.0;4];
    util_spatial::eig3(&mut value,&mut vector,&mut quat,&spread);
    let axes: [P3;3] = std::array::from_fn(|k| [vector[k],vector[3+k],vector[6+k]]);
    let extents = axes.map(|a| {
        let (lo,hi) = points.iter().fold((f64::INFINITY,f64::NEG_INFINITY),|(lo,hi),p| (lo.min(dot(*p,a)),hi.max(dot(*p,a))));
        hi-lo
    });
    let pad = 0.01*extents.iter().fold(1.0_f64,|m,&e| m.max(e));
    let mut solid = points.to_vec();
    for k in 0..3 {
        if extents[k] < pad { solid = solid.iter().flat_map(|&p| [madd(p,axes[k],-0.5*pad),madd(p,axes[k],0.5*pad)]).collect(); }
    }
    solid
}

// The builder's error does not say which hull it refused: compile each alone and name the body.
fn hull_error(spec: &ModelSpec, hulls: &[(usize,usize)]) -> Option<String> {
    for (mesh,&(body,k)) in spec.meshes.iter().zip(hulls) {
        let mut alone = ModelSpec::default();
        alone.meshes.push(mesh.clone());
        alone.bodies.push(BodySpec {
            name: Some("probe".into()), mass: 1.0, fullinertia: [1.0,1.0,1.0,0.0,0.0,0.0],
            joints: vec![JointSpec { r#type: JNT_FREE, limited: 0, ..JointSpec::default() }],
            geoms: vec![GeomSpec { r#type: GEOM_MESH, meshname: mesh.name.clone(), ..GeomSpec::default() }],
            ..BodySpec::default()
        });
        if let Err(e) = builder::compile(&alone) { return Some(format!("body {body} hull {k}: {e}")); }
    }
    None
}

fn joint_size(kind:i32) -> (usize,usize) {
    match kind {JNT_FREE=>(7,6),JNT_BALL=>(4,3),_=>(1,1)}
}
