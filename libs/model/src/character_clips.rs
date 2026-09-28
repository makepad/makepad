//! Procedural animation for generated characters. Clips are authored as
//! functions of time that build a pose with forward kinematics, limb aims
//! and analytic two-bone IK (feet planted on the ground, hands on a
//! weapon), then sampled into ordinary keyframes. Every clip is expressed in
//! body-relative terms (stride from leg length, crouch from hip height), so
//! one clip set fits every body the generator makes.
//!
//! Clip names follow the engine vocabularies: `idle`/`walk`/`run` for gait
//! blending, the platformer state names (`jump`, `fall`, `land`, `crouch`,
//! `dash`, `hang`, `ground_pound`, `wall_slide`, `backflip`, `long_jump`,
//! `knocked_down`), `holding-right` / `holding-right-shoot` for held
//! weapons, `aim_up`/`aim_down` for aim offsets and `reload`, plus hits,
//! deaths, emotes and fighting moves.
use super::body::{side_sign, Body};
use super::{CHARACTER_JOINTS, j};
use crate::character_mesh::{mix, norm, smooth01};
use crate::transform::*;
use crate::{AnimationChannel, AnimationClip, AnimationPath, Keyframe};
use std::f64::consts::{PI, TAU};

pub const CLIP_NAMES: &[&str] = &[
    "idle", "walk", "run", "sprint", "walk_back", "strafe_l", "strafe_r", "crouch", "crouch_move",
    "jump", "fall", "land", "holding-right", "holding-right-shoot", "aim_up", "aim_down", "reload",
    "hit_front", "hit_back", "death", "death_b", "wave", "cheer", "dance", "taunt", "salute",
    "fight_stance", "punch", "jab", "kick", "block", "knocked_down", "dash", "hang", "long_jump",
    "ground_pound", "wall_slide", "backflip", "drive", "sit",
];
/// Traversal clips (swing, zip, crawl, wall run, perch, glide, dive, launch,
/// vault, roll), authored upright in the body frame: a host tilts the body
/// onto walls and ceilings itself. Part of the default set for non-fighters.
pub const TRAVERSE_CLIP_NAMES: &[&str] = &["swing", "swing_l", "zip_pull", "crawl", "crawl_idle", "wall_run_l", "wall_run_r", "perch", "glide", "dive", "launch", "vault", "land_roll"];
/// Extra names for fighting games (see `ALIASES`); not in the default set.
pub const FIGHT_CLIP_NAMES: &[&str] = &["guard", "guard_low", "walk_fwd", "hit_high", "hit_mid", "hit_low", "down", "getup", "ko", "win", "lose", "intro", "backdash"];

type Q = [f64; 4];
const QI: Q = [0., 0., 0., 1.];
pub(crate) fn qx(deg: f64) -> Q { let h = deg.to_radians() * 0.5; [h.sin(), 0., 0., h.cos()] }
pub(crate) fn qy(deg: f64) -> Q { let h = deg.to_radians() * 0.5; [0., h.sin(), 0., h.cos()] }
pub(crate) fn qz(deg: f64) -> Q { let h = deg.to_radians() * 0.5; [0., 0., h.sin(), h.cos()] }
fn qaxis(axis: [f64; 3], deg: f64) -> Q { let a = norm(axis); let h = deg.to_radians() * 0.5; let s = h.sin(); [a[0] * s, a[1] * s, a[2] * s, h.cos()] }
pub(crate) fn qmul(a: Q, b: Q) -> Q { quat_mul(a, b) }
fn qinv(q: Q) -> Q { quat_inverse(q) }
fn qrot(q: Q, v: [f64; 3]) -> [f64; 3] { quat_rotate(q, v) }
fn qfrom_to(a: [f64; 3], b: [f64; 3]) -> Q {
    let (a, b) = (norm(a), norm(b));
    let d = dot(a, b);
    if d > 0.999999 { return QI; }
    if d < -0.999999 { let axis = if a[0].abs() < 0.9 { cross(a, [1., 0., 0.]) } else { cross(a, [0., 1., 0.]) }; return qaxis(axis, 180.); }
    let c = cross(a, b);
    let q = [c[0], c[1], c[2], 1. + d];
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    q.map(|v| v / l)
}
/// Rotation taking frame (a0, a1) to (b0, b1) (two orthonormal axes each).
fn qframe(a0: [f64; 3], a1: [f64; 3], b0: [f64; 3], b1: [f64; 3]) -> Q {
    let q1 = qfrom_to(a0, b0);
    let a1r = qrot(q1, a1);
    let b1p = norm(sub(b1, mul(b0, dot(b1, b0))));
    let a1p = norm(sub(a1r, mul(b0, dot(a1r, b0))));
    let ang = dot(cross(a1p, b1p), b0).atan2(dot(a1p, b1p));
    qmul(qaxis(b0, ang.to_degrees()), q1)
}
/// Rest data the clips need: world rest positions, parents and key sizes.
pub struct RigInfo {
    pub rest: Vec<[f64; 3]>,
    pub parent: Vec<Option<usize>>,
    pub u: f64,
    pub hand_frame: [[[f64; 3]; 3]; 2],
    pub hand_len: f64,
    pub palm_w: f64,
    pub leg_len: f64,
    pub hips_y: f64,
    pub ankle_y: f64,
    pub foot_fwd: f64,
    pub sh: f64,
    pub hh: f64,
}
impl RigInfo {
    pub(crate) fn new(b: &Body) -> RigInfo {
        let [hip, kn, an, ball, _, _] = b.leg[0];
        RigInfo {
            rest: b.joints.clone(), parent: CHARACTER_JOINTS.iter().map(|p| p.1).collect(), u: b.u,
            hand_frame: b.hand_frame, hand_len: b.hand_len, palm_w: b.palm_w,
            leg_len: length(sub(kn, hip)) + length(sub(an, kn)), hips_y: b.joints[0][1], ankle_y: an[1],
            foot_fwd: an[2] - ball[2], sh: b.sh, hh: b.hh,
        }
    }
}

/// A pose under construction: local rotations and the root position.
#[derive(Clone)]
pub struct Pose<'a> {
    rig: &'a RigInfo,
    pub local: Vec<Q>,
    pub root: [f64; 3],
    /// Local translation offsets (face joints: brows, mouth corners).
    pub offset: Vec<[f64; 3]>,
    g_rot: Vec<Q>,
    g_pos: Vec<[f64; 3]>,
    dirty: bool,
}
impl<'a> Pose<'a> {
    pub fn new(rig: &'a RigInfo) -> Self {
        let n = rig.rest.len();
        let mut p = Pose { rig, local: vec![QI; n], root: rig.rest[0], offset: vec![[0.; 3]; n], g_rot: vec![QI; n], g_pos: rig.rest.clone(), dirty: true };
        p.fk(); p
    }
    fn fk(&mut self) {
        if !self.dirty { return; }
        for i in 0..self.local.len() {
            match self.rig.parent[i] {
                None => { self.g_rot[i] = self.local[i]; self.g_pos[i] = self.root; }
                Some(p) => {
                    self.g_rot[i] = qmul(self.g_rot[p], self.local[i]);
                    self.g_pos[i] = add(self.g_pos[p], qrot(self.g_rot[p], sub(self.rig.rest[i], self.rig.rest[p])));
                }
            }
        }
        self.dirty = false;
    }
    /// Model-space (rotation, position) of joint `i`.
    pub fn global(&mut self, i: usize) -> (Q, [f64; 3]) { self.fk(); (self.g_rot[i], self.g_pos[i]) }
    pub fn pos(&mut self, name: &str) -> [f64; 3] { self.fk(); self.g_pos[j(name) as usize] }
    pub fn grot(&mut self, name: &str) -> Q { self.fk(); self.g_rot[j(name) as usize] }
    /// Multiply a local rotation onto a joint (applied in its parent frame).
    pub fn rot(&mut self, name: &str, q: Q) -> &mut Self { let i = j(name) as usize; self.local[i] = qmul(q, self.local[i]); self.dirty = true; self }
    pub fn set(&mut self, name: &str, q: Q) -> &mut Self { self.local[j(name) as usize] = q; self.dirty = true; self }
    pub fn shift(&mut self, d: [f64; 3]) -> &mut Self { self.root = add(self.root, d); self.dirty = true; self }
    /// Set a joint's global rotation (world-aligned rest frames).
    pub fn set_global(&mut self, name: &str, g: Q) {
        self.fk();
        let i = j(name) as usize;
        let pg = self.rig.parent[i].map_or(QI, |p| self.g_rot[p]);
        self.local[i] = qmul(qinv(pg), g);
        self.dirty = true;
    }
    /// Swing a bone (joint → child) to point along `dir` (world), with the
    /// smallest rotation from where its parent carries it.
    pub fn aim(&mut self, name: &str, child: &str, dir: [f64; 3]) {
        self.fk();
        let i = j(name) as usize; let c = j(child) as usize;
        let pg = self.rig.parent[i].map_or(QI, |p| self.g_rot[p]);
        let cur = qrot(pg, sub(self.rig.rest[c], self.rig.rest[i]));
        let g = qmul(qfrom_to(cur, dir), pg);
        self.local[i] = qmul(qinv(pg), g);
        self.dirty = true;
    }
    /// Two-bone IK: place `end` at `target` bending toward `pole` (world dir).
    pub fn ik(&mut self, upper: &str, lower: &str, end: &str, target: [f64; 3], pole: [f64; 3]) {
        self.fk();
        let (iu, il, ie) = (j(upper) as usize, j(lower) as usize, j(end) as usize);
        let l1 = length(sub(self.rig.rest[il], self.rig.rest[iu]));
        let l2 = length(sub(self.rig.rest[ie], self.rig.rest[il]));
        let a = self.g_pos[iu];
        let to = sub(target, a);
        let d = length(to).clamp((l1 - l2).abs() + 1e-4, l1 + l2 - 1e-4);
        let dt = norm(to);
        let pn = norm(sub(pole, mul(dt, dot(pole, dt))));
        let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2. * l1 * d)).clamp(-1., 1.);
        let knee = add(a, add(mul(dt, l1 * cos_a), mul(pn, l1 * (1. - cos_a * cos_a).sqrt())));
        self.aim(upper, lower, sub(knee, a));
        let end_pos = add(a, mul(dt, d));
        self.aim(lower, end, sub(end_pos, knee));
    }
    /// Foot on the ground: ankle target, pitch (+ = toe up) and toe bend.
    pub fn foot(&mut self, side: usize, ankle: [f64; 3], pitch: f64, toe: f64, yaw: f64) {
        let x = super::body::sfx(side);
        let sg = side_sign(side);
        let knee_dir = qrot(qy(yaw), [sg * 0.12, 0.1, -1.]);
        self.ik(&format!("upper_leg_{x}"), &format!("lower_leg_{x}"), &format!("foot_{x}"), ankle, knee_dir);
        self.set_global(&format!("foot_{x}"), qmul(qy(yaw), qx(pitch)));
        self.set(&format!("toe_{x}"), qx(toe));
    }
    /// Hand to a world target with the elbow toward `pole`, oriented by
    /// finger direction and palm normal (world).
    pub fn hand(&mut self, side: usize, target: [f64; 3], pole: [f64; 3], fingers: Option<([f64; 3], [f64; 3])>) {
        let x = super::body::sfx(side);
        self.ik(&format!("upper_arm_{x}"), &format!("lower_arm_{x}"), &format!("hand_{x}"), target, pole);
        if let Some((d, n)) = fingers {
            let [d0, n0, _] = self.rig.hand_frame[side];
            self.set_global(&format!("hand_{x}"), qframe(d0, n0, d, n));
        }
    }
    /// Arm hanging/swinging relative to the chest: `fwd` degrees forward,
    /// `out` degrees away from the body, elbow bend, wrist.
    pub fn arm(&mut self, side: usize, fwd: f64, out: f64, elbow: f64) {
        let x = super::body::sfx(side);
        let sg = side_sign(side);
        let chest = self.grot("chest");
        let (f, o) = (fwd.to_radians(), out.to_radians());
        let local_dir = [sg * o.sin(), -o.cos() * f.cos(), -o.cos() * f.sin()];
        self.aim(&format!("upper_arm_{x}"), &format!("lower_arm_{x}"), qrot(chest, local_dir));
        // Elbow bends forward/up around the arm's side axis.
        let ua = self.grot(&format!("upper_arm_{x}"));
        let d = norm(qrot(ua, sub(self.rig.rest[j(&format!("lower_arm_{x}")) as usize], self.rig.rest[j(&format!("upper_arm_{x}")) as usize])));
        let fwd_w = qrot(chest, [0., 0., -1.]);
        let axis = norm(cross(fwd_w, d));
        let axis = if length(axis) < 1e-6 { [1., 0., 0.] } else { axis };
        let fore = qrot(qaxis(axis, -elbow), d);
        let fore = if dot(fore, fwd_w) < dot(d, fwd_w) - 1e-6 { qrot(qaxis(axis, elbow), d) } else { fore };
        self.aim(&format!("lower_arm_{x}"), &format!("hand_{x}"), fore);
        // Keep the hand in line, palm toward the body.
        let la = self.grot(&format!("lower_arm_{x}"));
        let _ = la;
    }
    /// Curl fingers: 0 open … 1 fist; thumb and index separately.
    pub fn fingers(&mut self, side: usize, curl: f64, index: f64, thumb: f64) {
        let x = super::body::sfx(side);
        let [d, n, _] = self.rig.hand_frame[side];
        let axis = norm(cross(d, n));
        // Finger joints' rest frames are world aligned; the curl axis is the
        // hand's own side axis carried by the hand joint.
        for (name, amount, k1, k2) in [("fingers", curl, 70., 85.), ("index", index, 65., 80.), ("thumb", thumb, 25., 45.)] {
            let ax = if name == "thumb" { norm(add(axis, mul(d, 0.6))) } else { axis };
            self.set(&format!("{name}_1_{x}"), qaxis(ax, amount * k1));
            self.set(&format!("{name}_2_{x}"), qaxis(ax, amount * k2));
        }
    }
    pub fn look(&mut self, yaw: f64, pitch: f64) { self.rot("neck", qmul(qy(yaw * 0.4), qx(pitch * 0.4))); self.rot("head", qmul(qy(yaw * 0.6), qx(pitch * 0.6))); }
    pub fn blink(&mut self, amount: f64) { for x in ["l", "r"] { self.set(&format!("lid_{x}"), qx(-amount * 38.)); } }
    pub fn brows(&mut self, raise: f64, frown: f64) {
        // Rotation tilts the brow; translation carries it (a rotation about
        // its own pivot alone barely moves it).
        let h = self.rig.hh;
        for (x, sg) in [("l", -1.), ("r", 1.)] {
            let i = j(&format!("brow_{x}")) as usize;
            self.set(&format!("brow_{x}"), qz(sg * frown * 16.));
            self.offset[i] = [-sg * frown * 0.01 * h, (raise * 0.045 - frown * 0.012) * h, -frown * 0.012 * h];
        }
    }
    pub fn mouth(&mut self, open: f64, smile: f64) {
        self.set("jaw", qx(-open * 22.));
        let h = self.rig.hh;
        for (x, sg) in [("l", -1.), ("r", 1.)] {
            let i = j(&format!("mouth_{x}")) as usize;
            self.offset[i] = [sg * (smile.abs() * 0.018 + open * 0.01) * h, smile * 0.035 * h, 0.];
        }
    }
}

// ── sampling ───────────────────────────────────────────────────────────

fn sample(name: &str, rig: &RigInfo, duration: f64, fps: f64, looped: bool, force: &[&str], f: &dyn Fn(f64, &mut Pose)) -> AnimationClip {
    let n = ((duration * fps).round() as usize).max(1);
    let mut rows: Vec<(Vec<Q>, [f64; 3], Vec<[f64; 3]>)> = Vec::with_capacity(n + 1);
    for k in 0..=n {
        let t = if looped && k == n { 0. } else { duration * k as f64 / n as f64 };
        let mut p = Pose::new(rig);
        f(t, &mut p);
        rows.push((p.local.clone(), p.root, p.offset.clone()));
    }
    let force: Vec<usize> = force.iter().map(|n| j(n) as usize).collect();
    let mut channels = Vec::new();
    for ji in 0..rig.rest.len() {
        let moving = rows.iter().any(|r| { let q = r.0[ji]; (q[3].abs() - 1.).abs() > 1e-6 });
        if !moving && !force.contains(&ji) { continue; }
        // Keep quaternions in one hemisphere so linear keys never flip.
        let mut prev = QI;
        let keys: Vec<Keyframe> = rows.iter().enumerate().map(|(k, r)| {
            let mut q = r.0[ji];
            if q[0] * prev[0] + q[1] * prev[1] + q[2] * prev[2] + q[3] * prev[3] < 0. { q = q.map(|v| -v); }
            prev = q;
            Keyframe { time: duration * k as f64 / n as f64, value: q }
        }).collect();
        channels.push(AnimationChannel { joint: ji as u32, path: AnimationPath::Rotation, keys });
    }
    for ji in 1..rig.rest.len() {
        if !rows.iter().any(|r| r.2[ji] != [0.; 3]) { continue; }
        let base = sub(rig.rest[ji], rig.rest[rig.parent[ji].unwrap_or(0)]);
        channels.push(AnimationChannel { joint: ji as u32, path: AnimationPath::Translation, keys: rows.iter().enumerate().map(|(k, r)| { let o = r.2[ji]; Keyframe { time: duration * k as f64 / n as f64, value: [base[0] + o[0], base[1] + o[1], base[2] + o[2], 0.] } }).collect() });
    }
    channels.push(AnimationChannel { joint: 0, path: AnimationPath::Translation, keys: rows.iter().enumerate().map(|(k, r)| Keyframe { time: duration * k as f64 / n as f64, value: [r.1[0], r.1[1], r.1[2], 0.] }).collect() });
    AnimationClip { name: name.into(), channels }
}

/// Catmull-Rom through (time, value) keys, clamped at the ends.
pub(crate) fn curve(t: f64, keys: &[(f64, f64)]) -> f64 {
    if t <= keys[0].0 { return keys[0].1; }
    let n = keys.len();
    if t >= keys[n - 1].0 { return keys[n - 1].1; }
    let i = (0..n - 1).find(|&i| t < keys[i + 1].0).unwrap_or(n - 2);
    let (t0, t1) = (keys[i].0, keys[i + 1].0);
    let s = (t - t0) / (t1 - t0);
    let p0 = keys[i.saturating_sub(1)].1; let p1 = keys[i].1; let p2 = keys[i + 1].1; let p3 = keys[(i + 2).min(n - 1)].1;
    let (s2, s3) = (s * s, s * s * s);
    0.5 * (2. * p1 + (-p0 + p2) * s + (2. * p0 - 5. * p1 + 4. * p2 - p3) * s2 + (-p0 + 3. * p1 - 3. * p2 + p3) * s3)
}
fn ease(t: f64) -> f64 { let t = t.clamp(0., 1.); t * t * (3. - 2. * t) }

// ── shared pieces ─────────────────────────────────────────────────────

/// Relaxed standing arms and hands.
fn relaxed(p: &mut Pose, swing: f64) {
    // Arms hang close with a soft elbow; the A-pose rest is never shown.
    for side in 0..2 { p.arm(side, 8. + swing * if side == 0 { 1. } else { -1. }, 5., 30.); p.fingers(side, 0.45, 0.35, 0.25); }
}
fn feet_planted(p: &mut Pose, spread: f64, crouch: f64, lean_knees: f64) {
    let r = p.rig;
    for side in 0..2 {
        let x = super::body::sfx(side);
        let a = r.rest[j(&format!("foot_{x}")) as usize];
        let sg = side_sign(side);
        p.foot(side, [a[0] + sg * spread, a[1], a[2] - lean_knees * 0.2 * crouch], 0., 0., sg * 6.);
    }
}

struct Gait { cycle: f64, stride: f64, duty: f64, lift: f64, bob: f64, lean: f64, arm: f64, elbow: f64, twist: f64, crouch: f64, side: f64, back: bool, knee_up: f64 }

/// Foot-planted locomotion: stance feet slide back under the body at the
/// travel speed, swing feet arc forward, legs solve by IK.
fn gait(t: f64, p: &mut Pose, g: &Gait) {
    let r = p.rig;
    let u = r.u;
    let ph = (t / g.cycle).rem_euclid(1.);
    let dir: [f64; 3] = if g.side != 0. { [g.side, 0., 0.] } else if g.back { [0., 0., 1.] } else { [0., 0., -1.] };
    // Hips: bob, sway toward the stance foot, twist with the swing leg.
    let d2 = g.duty * 0.5;
    let bob = -g.bob * u * (0.5 + 0.5 * (4. * PI * (ph - d2)).cos());
    let sway = -0.012 * u * (TAU * (ph - d2)).cos() * if g.side != 0. { 0.3 } else { 1. };
    p.shift([sway, bob - g.crouch * r.leg_len, 0.]);
    let twist = g.twist * (TAU * ph).sin() * if g.back { -1. } else { 1. };
    p.rot("hips", qmul(qy(twist), qz(-sway / u * 60.)));
    p.rot("spine", qx(-g.lean * 0.5));
    p.rot("chest", qmul(qy(-twist * 1.5), qx(-g.lean * 0.5)));
    p.look(twist * 0.5, g.lean * 0.6 + g.bob * 40. * (4. * PI * (ph - d2)).cos() * 0.5);
    for side in 0..2 {
        let x = super::body::sfx(side);
        let sg = side_sign(side);
        let a = r.rest[j(&format!("foot_{x}")) as usize];
        let fp = (ph + side as f64 * 0.5).rem_euclid(1.);
        let (off, lift, pitch, toe);
        if fp < g.duty {
            let s = fp / g.duty;
            off = g.stride * (0.5 - s);
            lift = 0.;
            pitch = curve(s, &[(0., 14.), (0.16, 0.), (0.62, 0.), (1., -34.)]) * if g.side != 0. { 0.3 } else { 1. };
            toe = curve(s, &[(0., 0.), (0.62, 0.), (1., 32.)]) * if g.side != 0. { 0.3 } else { 1. };
        } else {
            let s = (fp - g.duty) / (1. - g.duty);
            off = g.stride * (-0.5 + ease(s));
            lift = g.lift * u * (PI * s.powf(0.75)).sin() + g.knee_up * u * (PI * s).sin().powi(2) * (1. - s);
            pitch = curve(s, &[(0., -34.), (0.35, -20.), (0.8, 6.), (1., 14.)]) * if g.side != 0. { 0.3 } else { 1. };
            toe = curve(s, &[(0., 32.), (0.3, 8.), (1., 0.)]);
        }
        let heel_rise = if pitch < 0. { r.foot_fwd * 0.8 * (-pitch).to_radians().sin() } else { 0. };
        let target = add([a[0] + sg * 0.004 * u, a[1] + lift + heel_rise, a[2]], mul(dir, -off));
        let target = add(target, [0., 0., if g.side != 0. { 0. } else { 0.01 * u }]);
        p.foot(side, target, pitch, toe, sg * 5.);
    }
    // Arms counter-swing with a little lag.
    for side in 0..2 {
        let arm_ph = (ph + side as f64 * 0.5 - 0.06).rem_euclid(1.);
        let swing = g.arm * -(TAU * arm_ph).cos() * if g.back { -0.6 } else { 1. } * if g.side != 0. { 0.35 } else { 1. };
        let elbow = g.elbow + 0.4 * g.elbow * swing.max(0.) / g.arm.max(1.);
        p.arm(side, swing + g.lean * 0.4, 10., elbow);
        p.fingers(side, if g.elbow > 60. { 0.75 } else { 0.35 }, 0.3, 0.3);
    }
}

// ── the clip set ──────────────────────────────────────────────────────

/// Versus-kit state names served by an existing clip.
const ALIASES: &[(&str, &str)] = &[("guard", "block"), ("guard_low", "crouch"), ("walk_fwd", "walk"), ("hit_high", "hit_front"), ("hit_mid", "hit_front"),
    ("hit_low", "hit_back"), ("down", "knocked_down"), ("ko", "death"), ("win", "cheer"), ("lose", "death_b"), ("intro", "taunt"), ("backdash", "walk_back")];

pub fn clip(name: &str, rig: &RigInfo) -> Option<AnimationClip> {
    if let Some((_, base)) = ALIASES.iter().find(|(a, _)| *a == name) {
        let mut c = clip(base, rig)?;
        c.name = name.into();
        return Some(c);
    }
    if name == "getup" {
        // The knockdown played backwards.
        let mut c = clip("knocked_down", rig)?;
        let d = c.channels.iter().flat_map(|ch| ch.keys.last()).map(|k| k.time).fold(0., f64::max);
        for ch in &mut c.channels { ch.keys.reverse(); for k in &mut ch.keys { k.time = d - k.time; } }
        c.name = name.into();
        return Some(c);
    }
    let u = rig.u;
    let leg = rig.leg_len / u;
    let walk = Gait { cycle: 1.05, stride: 0.62 * rig.leg_len, duty: 0.6, lift: 0.07, bob: 0.018, lean: 3., arm: 18., elbow: 16., twist: 6., crouch: 0.02, side: 0., back: false, knee_up: 0. };
    let run = Gait { cycle: 0.68, stride: 1.25 * rig.leg_len, duty: 0.38, lift: 0.1, bob: 0.03, lean: 10., arm: 34., elbow: 78., twist: 9., crouch: 0.04, side: 0., back: false, knee_up: 0.14 };
    let _ = leg;
    Some(match name {
        "idle" => sample(name, rig, 4.0, 12., true, &[], &|t, p| {
            let b = (TAU * t / 4.).sin();
            p.shift([0.008 * u * (TAU * t / 4.).sin(), -0.004 * u * (0.5 + 0.5 * (TAU * t / 2.).cos()), 0.]);
            p.rot("hips", qz(1.2 * b));
            p.rot("spine", qx(-1. + 0.6 * (TAU * t / 2.).sin()));
            p.rot("chest", qmul(qx(0.8 * (TAU * t / 2.).sin()), qz(-1.4 * b)));
            p.look(8. * (TAU * t / 4. + 0.8).sin() * smooth01(0.8, 1.6, t) * (1. - smooth01(3.0, 3.8, t)), -2.);
            relaxed(p, 1.5 * (TAU * t / 2.).sin());
            // Weight on the right leg: hip drops left, left knee soft.
            p.rot("hips", qmul(qz(-5.), qy(4.)));
            feet_planted(p, 0.018 * u, 0., 0.);
            p.rot("lower_leg_l", qx(-6.));
            let blink = curve(t, &[(1.2, 0.), (1.28, 1.), (1.36, 0.)]).max(curve(t, &[(3.3, 0.), (3.38, 1.), (3.46, 0.)]));
            p.blink(blink);
            p.mouth(0., 0.15);
        }),
        "walk" => sample(name, rig, walk.cycle, 24., true, &[], &|t, p| { gait(t, p, &walk); }),
        "run" => sample(name, rig, run.cycle, 32., true, &[], &|t, p| { gait(t, p, &run); }),
        "sprint" => { let g = Gait { cycle: 0.58, stride: 1.55 * rig.leg_len, duty: 0.33, lift: 0.12, bob: 0.034, lean: 18., arm: 46., elbow: 88., twist: 11., crouch: 0.05, side: 0., back: false, knee_up: 0.2 }; sample(name, rig, g.cycle, 32., true, &[], &move |t, p| gait(t, p, &g)) }
        "walk_back" => { let g = Gait { back: true, stride: 0.5 * rig.leg_len, cycle: 0.95, lean: -2., arm: 10., ..Gait { ..walk_clone(&walk) } }; sample(name, rig, g.cycle, 24., true, &[], &move |t, p| gait(t, p, &g)) }
        "strafe_l" | "strafe_r" => {
            let s = if name == "strafe_l" { -1. } else { 1. };
            let g = Gait { cycle: 0.72, stride: 0.75 * rig.leg_len, duty: 0.45, lift: 0.07, bob: 0.02, lean: 5., arm: 12., elbow: 40., twist: 2., crouch: 0.04, side: s, back: false, knee_up: 0.02 };
            sample(name, rig, g.cycle, 24., true, &[], &move |t, p| { gait(t, p, &g); p.rot("spine", qz(-s * 3.)); })
        }
        "crouch" => sample(name, rig, 2.4, 10., true, &[], &|t, p| {
            let br = (TAU * t / 2.4).sin();
            p.shift([0., -0.3 * rig.leg_len - 0.004 * u * br, 0.02 * u]);
            p.rot("hips", qx(-8.));
            p.rot("spine", qx(-10. - 1. * br));
            p.rot("chest", qx(-6.));
            p.look(0., 14.);
            for side in 0..2 { p.arm(side, 30., 12., 50.); p.fingers(side, 0.5, 0.4, 0.3); }
            feet_planted(p, 0.035 * u, 1., 0.);
        }),
        "crouch_move" => { let g = Gait { cycle: 1.0, stride: 0.55 * rig.leg_len, duty: 0.62, lift: 0.06, bob: 0.012, lean: 18., arm: 12., elbow: 50., twist: 5., crouch: 0.28, side: 0., back: false, knee_up: 0. }; sample(name, rig, g.cycle, 24., true, &[], &move |t, p| { gait(t, p, &g); p.look(0., -6.); }) }
        "jump" => sample(name, rig, 0.5, 30., false, &[], &|t, p| {
            let dip = curve(t, &[(0., 0.), (0.1, -0.1), (0.16, -0.12), (0.24, 0.03), (0.34, 0.06), (0.5, 0.05)]);
            let tuck = curve(t, &[(0., 0.), (0.22, 0.), (0.34, 0.2), (0.5, 0.28)]);
            p.shift([0., dip * u, 0.]);
            let lean = curve(t, &[(0., 0.), (0.12, 14.), (0.24, -6.), (0.5, 4.)]);
            p.rot("spine", qx(-lean * 0.5)); p.rot("chest", qx(-lean * 0.5));
            p.look(0., curve(t, &[(0., 0.), (0.14, 10.), (0.3, -12.), (0.5, -6.)]));
            // Arms swing back on the dip, then drive up and out with bent elbows.
            // Symmetric: up and out in a V, elbows nearly straight so the
            // bend direction never flips on the raised arm.
            let arm = curve(t, &[(0., 5.), (0.12, -45.), (0.24, 25.), (0.36, 20.), (0.5, 15.)]);
            let out = curve(t, &[(0., 10.), (0.12, 20.), (0.24, 110.), (0.36, 145.), (0.5, 135.)]);
            for side in 0..2 { p.arm(side, arm, out, curve(t, &[(0., 15.), (0.14, 25.), (0.3, 20.), (0.5, 25.)])); p.fingers(side, 0.5, 0.4, 0.3); }
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                let ground = [a[0], a[1], a[2]];
                let airborne = smooth01(0.2, 0.3, t);
                // In the air the feet hang under the hips, knees tucking up.
                let hang = [a[0] + sg * 0.01 * u, hips[1] - (rig.hips_y - a[1]) + (tuck + 0.02) * u + if side == 0 { 0.06 * u * airborne } else { 0. }, a[2] - if side == 0 { 0.08 * u } else { -0.03 * u }];
                let target = lerp(ground, hang, airborne);
                let pitch = curve(t, &[(0., 0.), (0.16, 0.), (0.24, -40.), (0.34, -25.), (0.5, -15.)]);
                p.foot(side, target, pitch, curve(t, &[(0.16, 0.), (0.24, 30.), (0.4, 5.)]), sg * 6.);
            }
            p.brows(curve(t, &[(0., 0.), (0.25, 0.6), (0.5, 0.4)]), 0.);
            p.mouth(curve(t, &[(0., 0.), (0.24, 0.35), (0.5, 0.2)]), 0.2);
        }),
        "fall" => sample(name, rig, 1.2, 15., true, &[], &|t, p| {
            let w = (TAU * t / 1.2).sin(); let w2 = (TAU * 2. * t / 1.2).sin();
            p.shift([0., 0.05 * u, 0.]);
            p.rot("spine", qx(3. + 2. * w)); p.rot("chest", qz(3. * w2));
            p.look(0., -10.);
            for side in 0..2 { let sg = side_sign(side); p.arm(side, 70. + 12. * w * sg, 55. + 10. * w2, 35. + 10. * w); p.fingers(side, 0.2, 0.1, 0.2); }
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                let fwd = if side == 0 { -0.07 } else { 0.04 } * u + 0.02 * u * w * sg;
                let target = [a[0] + sg * 0.02 * u, hips[1] - (rig.hips_y - a[1]) + 0.12 * u + if side == 0 { 0.05 * u } else { 0. }, a[2] + fwd];
                p.foot(side, target, -20., 10., sg * 8.);
            }
            p.mouth(0.35, -0.1); p.brows(0.8, 0.);
        }),
        "land" => sample(name, rig, 0.5, 30., false, &[], &|t, p| {
            let dip = curve(t, &[(0., -0.02), (0.07, -0.16), (0.16, -0.12), (0.3, -0.02), (0.4, 0.008), (0.5, 0.)]);
            p.shift([0., dip * u, 0.]);
            let lean = curve(t, &[(0., 4.), (0.08, 18.), (0.25, 8.), (0.5, 0.)]);
            p.rot("hips", qx(-lean * 0.3)); p.rot("spine", qx(-lean * 0.4)); p.rot("chest", qx(-lean * 0.3));
            p.look(0., curve(t, &[(0., -6.), (0.08, 12.), (0.3, 2.), (0.5, 0.)]));
            let arm = curve(t, &[(0., 90.), (0.07, 40.), (0.2, 25.), (0.5, 5.)]);
            for side in 0..2 { p.arm(side, arm, curve(t, &[(0., 40.), (0.1, 30.), (0.5, 9.)]), curve(t, &[(0., 35.), (0.1, 45.), (0.5, 15.)])); p.fingers(side, 0.3, 0.2, 0.2); }
            feet_planted(p, 0.02 * u, 1., 0.);
            p.blink(curve(t, &[(0.04, 0.), (0.08, 0.8), (0.16, 0.)]));
        }),
        "holding-right" | "aim_up" | "aim_down" => {
            let pitch = match name { "aim_up" => 1., "aim_down" => -1., _ => 0. };
            sample(name, rig, 0.1, 10., false, &HOLD_FORCE, &move |_, p| hold_pose(p, pitch, 0.))
        }
        "holding-right-shoot" => sample(name, rig, 0.24, 30., false, &HOLD_FORCE, &|t, p| {
            let k = curve(t, &[(0., 0.), (0.03, 1.), (0.09, 0.55), (0.24, 0.)]);
            hold_pose(p, 0., k);
            p.blink(curve(t, &[(0., 0.), (0.03, 0.6), (0.1, 0.)]));
        }),
        "reload" => sample(name, rig, 2.2, 20., false, &HOLD_FORCE, &|t, p| reload_pose(p, t)),
        "hit_front" | "hit_back" => {
            let s = if name == "hit_front" { 1. } else { -1. };
            sample(name, rig, 0.45, 30., false, &[], &move |t, p| {
                let k = curve(t, &[(0., 0.), (0.05, 1.), (0.16, 0.7), (0.45, 0.)]);
                p.shift([0., -0.03 * u * k, s * 0.03 * u * k]);
                p.rot("spine", qx(s * 9. * k)); p.rot("chest", qmul(qx(s * 8. * k), qz(4. * k)));
                p.look(-10. * k, s * 14. * k);
                for side in 0..2 { p.arm(side, 20. * k + 4., 9. + 22. * k, 14. + 50. * k); p.fingers(side, 0.3 + 0.5 * k, 0.3, 0.3); }
                feet_planted(p, 0.012 * u, 0., 0.);
                // Squint, frown and a wide grimace.
                p.blink(0.65 * k); p.brows(-0.4 * k, 1.0 * k); p.mouth(1.1 * k, -0.8 * k);
            })
        }
        "death" | "death_b" => {
            let back = name == "death";
            sample(name, rig, 1.5, 24., false, &[], &move |t, p| death_pose(p, t, back))
        }
        "knocked_down" => sample(name, rig, 1.0, 20., false, &[], &|t, p| death_pose(p, (t * 1.4).min(1.5), true)),
        "wave" => sample(name, rig, 2.0, 20., true, &[], &|t, p| {
            relaxed(p, 0.);
            feet_planted(p, 0.012 * u, 0., 0.);
            let w = (TAU * t / 0.5).sin();
            let up = smooth01(0., 0.3, t) * (1. - smooth01(1.7, 2., t));
            p.rot("chest", qz(-3. * up));
            p.arm(1, 20. * up + 4., mix(9., 150., up), mix(14., 30. + 25. * w, up));
            p.fingers(1, 0.05, 0.05, 0.1);
            p.look(4. * w * up, -4. * up);
            p.mouth(0.25 * up, 0.9 * up); p.brows(0.6 * up, 0.);
        }),
        "cheer" => sample(name, rig, 1.6, 20., true, &[], &|t, p| {
            let h = (TAU * t / 0.8).sin();
            let hop = (h * 0.5 + 0.5).powi(2);
            p.shift([0., 0.05 * u * hop - 0.03 * u, 0.]);
            p.rot("spine", qx(4. * hop)); p.look(0., -12.);
            for side in 0..2 { p.arm(side, 30. - 10. * h, 150. - 20. * h, 30. + 20. * h); p.fingers(side, 0.9, 0.9, 0.7); }
            feet_planted(p, 0.03 * u, 0.5, 0.);
            p.mouth(0.7, 1.); p.brows(1., 0.);
        }),
        "dance" => sample(name, rig, 2.0, 20., true, &[], &|t, p| {
            let b = (TAU * t / 0.5).sin(); let s = (TAU * t / 1.0).sin();
            p.shift([0.04 * u * s, -0.03 * u * (b * 0.5 + 0.5), 0.]);
            p.rot("hips", qmul(qz(8. * s), qy(12. * s)));
            p.rot("chest", qmul(qz(-12. * s), qy(-14. * s)));
            p.look(10. * s, -6. * b);
            p.arm(0, 60. + 30. * s, 40. + 20. * b, 80. + 20. * s);
            p.arm(1, 60. - 30. * s, 40. - 20. * b, 80. - 20. * s);
            for side in 0..2 { p.fingers(side, 0.6, 0.1, 0.2); }
            feet_planted(p, 0.05 * u, 1., 0.);
            p.mouth(0.2, 1.); p.brows(0.3, 0.);
        }),
        "taunt" => sample(name, rig, 2.0, 20., false, &[], &|t, p| {
            let k = smooth01(0., 0.35, t) * (1. - smooth01(1.6, 2., t));
            let pulse = 1. + 0.15 * (TAU * t / 0.4).sin() * k;
            p.shift([0., -0.03 * u * k, 0.]);
            p.rot("spine", qx(4. * k)); p.rot("chest", qx(6. * k * pulse));
            for side in 0..2 { p.arm(side, mix(4., 10., k), mix(9., 85., k), mix(14., 125. * pulse.min(1.05), k)); p.fingers(side, mix(0.3, 1., k), mix(0.2, 1., k), mix(0.2, 0.8, k)); }
            feet_planted(p, 0.05 * u * k + 0.012 * u, 0.8 * k, 0.);
            p.look(0., -10. * k); p.mouth(0.3 * k, 0.7 * k); p.brows(-0.3 * k, 0.9 * k);
        }),
        "salute" => sample(name, rig, 1.6, 20., false, &[], &|t, p| {
            relaxed(p, 0.); feet_planted(p, 0.01 * u, 0., 0.);
            let k = smooth01(0., 0.35, t) * (1. - smooth01(1.2, 1.6, t));
            let brow = add(p.pos("head"), [0.05 * u, 0.06 * u, -0.1 * u]);
            let rest = p.pos("hand_r");
            let target = lerp(rest, brow, k);
            p.hand(1, target, [1., -0.4, 0.3], if k > 0.01 { Some((norm([-0.8, 0.3, -0.1]), norm([0.1, -0.1, -1.]))) } else { None });
            p.fingers(1, 0., 0., 0.); p.look(0., -3. * k); p.mouth(0., 0.1);
        }),
        "fight_stance" | "block" => {
            let block = name == "block";
            sample(name, rig, 1.0, 20., true, &[], &move |t, p| fight_stance(p, t, if block { 1. } else { 0. }))
        }
        "punch" | "jab" => {
            let right = name == "punch";
            sample(name, rig, if right { 0.5 } else { 0.36 }, 30., false, &[], &move |t, p| punch_pose(p, t, right))
        }
        "kick" => sample(name, rig, 0.7, 30., false, &[], &|t, p| kick_pose(p, t)),
        "dash" => sample(name, rig, 0.4, 30., false, &[], &|t, p| {
            let k = curve(t, &[(0., 0.), (0.08, 1.), (0.3, 1.), (0.4, 0.6)]);
            let g = Gait { cycle: 0.8, stride: 1.4 * rig.leg_len, duty: 0.35, lift: 0.1, bob: 0.02, lean: 26. * k, arm: 0., elbow: 60., twist: 0., crouch: 0.1, side: 0., back: false, knee_up: 0.15 };
            gait(0.1 + t * 0.2, p, &g);
            for side in 0..2 { p.arm(side, -50. * k, 25., 20.); }
            p.look(0., 20. * k); p.mouth(0.2, 0.3); p.brows(0., 0.5);
        }),
        "hang" => sample(name, rig, 1.6, 15., true, &[], &|t, p| {
            let s = (TAU * t / 1.6).sin();
            p.shift([0., -0.02 * u, 0.02 * u]);
            let top = rig.sh + 0.55 * rig.leg_len * 0.62;
            for side in 0..2 {
                let sg = side_sign(side);
                p.hand(side, [sg * 0.18 * u, top, -0.12 * u], [sg, 0., 0.3], Some(([0., 1., -0.2], [0., 0., 1.])));
                p.fingers(side, 0.9, 0.9, 0.6);
            }
            p.look(0., -18.);
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                p.foot(side, [a[0], hips[1] - (rig.hips_y - a[1]) + 0.06 * u + 0.02 * u * s * sg, a[2] - 0.04 * u + 0.03 * u * s * sg], -25., 5., sg * 5.);
            }
        }),
        "long_jump" => sample(name, rig, 0.6, 30., false, &[], &|t, p| {
            p.rot("hips", qx(-12.)); p.rot("spine", qx(-14.)); p.rot("chest", qx(-8.)); p.look(0., 22.);
            let k = smooth01(0., 0.25, t);
            for side in 0..2 { p.arm(side, mix(10., 150., k), 25., 15.); p.fingers(side, 0.1, 0.1, 0.1); }
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                p.foot(side, [a[0], hips[1] - (rig.hips_y - a[1]) + 0.1 * u, a[2] + 0.18 * u * k + if side == 0 { 0.03 * u } else { 0. }], -40., 20., sg * 4.);
            }
            p.mouth(0.3, 0.6);
        }),
        "ground_pound" => sample(name, rig, 0.5, 30., false, &[], &|t, p| {
            let spin = curve(t, &[(0., 0.), (0.25, 360.), (0.5, 360.)]);
            p.rot("hips", qx(-spin * 0.0 - 10. * smooth01(0.2, 0.3, t)));
            p.shift([0., 0.06 * u, 0.]);
            p.rot("spine", qx(-16.)); p.rot("chest", qx(-10.)); p.look(0., 25.);
            for side in 0..2 { p.arm(side, 20., 40., 100.); p.fingers(side, 1., 1., 0.8); }
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                p.foot(side, [a[0] + sg * 0.03 * u, hips[1] - 0.35 * rig.leg_len, a[2] - 0.1 * u], -30., 10., sg * 10.);
            }
            p.brows(-0.2, 1.); p.mouth(0.1, -0.5);
        }),
        "wall_slide" => sample(name, rig, 1.0, 15., true, &[], &|t, p| {
            let s = (TAU * t).sin();
            p.rot("spine", qz(8.)); p.look(-25., -5.);
            p.arm(0, 70. + 5. * s, 70., 30.); p.arm(1, 20., 50., 60.);
            for side in 0..2 { p.fingers(side, 0.2, 0.1, 0.2); }
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                p.foot(side, [a[0] - 0.05 * u, hips[1] - (rig.hips_y - a[1]) + if side == 0 { 0.2 * u } else { 0.04 * u }, a[2]], -10., 5., sg * 5.);
            }
            p.brows(0.4, 0.3);
        }),
        "backflip" => sample(name, rig, 0.8, 30., false, &[], &|t, p| {
            let spin = curve(t, &[(0., 0.), (0.1, 5.), (0.6, 350.), (0.8, 360.)]);
            let tuck = smooth01(0.08, 0.25, t) * (1. - smooth01(0.55, 0.75, t));
            p.set("hips", qx(spin));
            p.shift([0., 0.12 * u * (PI * t / 0.8).sin(), 0.]);
            p.rot("spine", qx(-25. * tuck));
            for side in 0..2 { p.arm(side, 40. + 60. * tuck, 20., 90. * tuck + 15.); }
            for side in 0..2 {
                let x = super::body::sfx(side);
                let (ul, ll) = (format!("upper_leg_{x}"), format!("lower_leg_{x}"));
                p.set(&ul, qx(100. * tuck)); p.set(&ll, qx(-120. * tuck));
            }
            p.mouth(0.3, 0.8);
        }),
        "swing" | "swing_l" => {
            // Hanging from one raised hand, legs together swinging ±25°.
            let hand = if name == "swing" { 1 } else { 0 };
            sample(name, rig, 1.2, 20., true, &[], &move |t, p| {
                let sw = (TAU * t / 1.2).sin();
                let sg = side_sign(hand);
                p.shift([0., 0.02 * u, 0.]);
                p.rot("hips", qx(8. * sw));
                p.rot("chest", qmul(qz(-sg * 8.), qx(-4. * sw)));
                p.hand(hand, [sg * 0.1 * u, rig.sh + 0.55 * u, -0.08 * u], [sg, 0.2, 0.3], Some(([0., 1., 0.], [0., 0., -1.])));
                p.fingers(hand, 1., 1., 0.8);
                p.arm(1 - hand, 15. + 10. * sw, 75., 25.); p.fingers(1 - hand, 0.3, 0.2, 0.2);
                for side in 0..2 { let x = super::body::sfx(side); p.set(&format!("upper_leg_{x}"), qx(10. + 25. * sw)); p.set(&format!("lower_leg_{x}"), qx(-15. - 8. * sw.max(0.))); p.set(&format!("foot_{x}"), qx(-20.)); }
                p.look(0., -6. * sw); p.mouth(0.15, 0.2); p.brows(0.4, 0.2);
            })
        }
        "zip_pull" => sample(name, rig, 0.5, 10., true, &[], &|t, p| {
            let b = (TAU * t / 0.5).sin();
            p.rot("hips", qx(-20.)); p.rot("chest", qx(-6.));
            for side in 0..2 {
                let sg = side_sign(side);
                p.hand(side, [sg * 0.09 * u, rig.sh + 0.42 * u + 0.01 * u * b, -0.12 * u], [sg, 0., 0.3], Some(([0., 1., 0.], [0., 0., -1.])));
                p.fingers(side, 1., 1., 0.8);
                let x = super::body::sfx(side);
                p.set(&format!("upper_leg_{x}"), qx(62. + 4. * b)); p.set(&format!("lower_leg_{x}"), qx(-95.)); p.set(&format!("foot_{x}"), qx(-15.));
            }
            p.look(0., 14.); p.brows(-0.2, 0.7); p.mouth(0.1, -0.3);
        }),
        "crawl" | "crawl_idle" => {
            let moving = name == "crawl";
            sample(name, rig, 0.8, 24., true, &[], &move |t, p| {
                // Quadruped: chest low, diagonal pairs (left hand + right
                // foot) plant together, head up.
                let ph = if moving { t / 0.8 } else { 0. };
                let stride = if moving { 0.3 * u } else { 0. };
                let low = rig.hips_y * 0.52;
                p.shift([0., -(rig.hips_y - low), 0.08 * u]);
                p.rot("hips", qx(-62.)); p.rot("spine", qx(-8.)); p.rot("chest", qx(-4.));
                p.look(0., 62.);
                for side in 0..2 {
                    let sg = side_sign(side);
                    let hand_ph = (ph + side as f64 * 0.5).rem_euclid(1.);
                    let foot_ph = (ph + (1 - side) as f64 * 0.5).rem_euclid(1.);
                    let step = |q: f64| -> (f64, f64) { if q < 0.6 { (stride * (0.5 - q / 0.6), 0.) } else { let s = (q - 0.6) / 0.4; (stride * (-0.5 + ease(s)), 0.06 * u * (PI * s).sin()) } };
                    let (hz, hy) = step(hand_ph);
                    p.hand(side, [sg * 0.17 * u, 0.02 * u + hy, -0.62 * u + hz], [sg * 0.6, -0.2, 1.], Some(([0., 0., -1.], [0., -1., 0.])));
                    p.fingers(side, 0.15, 0.1, 0.1);
                    let (fz, fy) = step(foot_ph);
                    let x = super::body::sfx(side);
                    let a = rig.rest[j(&format!("foot_{x}")) as usize];
                    p.foot(side, [a[0] + sg * 0.03 * u, a[1] + fy + 0.01 * u, a[2] + 0.22 * u + fz], -45., 35., sg * 8.);
                }
                p.brows(-0.1, 0.5);
            })
        }
        "wall_run_l" | "wall_run_r" => {
            let s = if name == "wall_run_l" { 1. } else { -1. };
            let g = Gait { cycle: 0.6, stride: 1.4 * rig.leg_len, duty: 0.35, lift: 0.1, bob: 0.03, lean: 14., arm: 42., elbow: 85., twist: 10., crouch: 0.05, side: 0., back: false, knee_up: 0.18 };
            sample(name, rig, g.cycle, 32., true, &[], &move |t, p| { gait(t, p, &g); p.rot("hips", qz(s * 15.)); p.look(0., 0.); })
        }
        "perch" => sample(name, rig, 2.0, 10., true, &[], &|t, p| {
            let br = (TAU * t / 2.).sin();
            p.shift([0., -0.5 * rig.leg_len - 0.004 * u * br, 0.06 * u]);
            p.rot("hips", qx(-24.)); p.rot("spine", qx(-14.)); p.rot("chest", qx(-6.));
            p.look(0., 38.);
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                p.foot(side, [a[0] + sg * 0.05 * u, a[1] + 0.05 * u, a[2] + 0.04 * u], -35., 40., sg * 14.);
            }
            p.hand(1, [0.04 * u, 0.03 * u, -0.2 * u], [1., 0., 0.5], Some(([0., -0.3, -1.], [0., -1., 0.])));
            p.fingers(1, 0.2, 0.1, 0.1);
            p.arm(0, 30., 25., 70.); p.fingers(0, 0.4, 0.3, 0.3);
            p.brows(0.1, 0.4);
        }),
        "glide" | "dive" => {
            let glide = name == "glide";
            sample(name, rig, 1.0, 10., true, &[], &move |t, p| {
                let w = (TAU * t).sin();
                p.rot("hips", qx(-85.));
                p.look(0., if glide { 70. } else { 40. });
                for side in 0..2 {
                    if glide { p.arm(side, -25. + 3. * w, 88., 6.); p.fingers(side, 0.05, 0.05, 0.1); } else { p.arm(side, -8., 4., 2.); p.fingers(side, 0.2, 0.1, 0.1); }
                    let x = super::body::sfx(side);
                    p.set(&format!("upper_leg_{x}"), qz(if side == 0 { 2. } else { -2. })); p.set(&format!("foot_{x}"), qx(-40.));
                }
                p.mouth(0.1, 0.3); p.brows(0.3, 0.);
            })
        }
        "launch" => sample(name, rig, 0.4, 30., false, &[], &|t, p| {
            let crouch = curve(t, &[(0., 0.18), (0.08, 0.28), (0.18, 0.02), (0.4, -0.05)]);
            p.shift([0., -crouch * rig.leg_len, 0.]);
            p.rot("spine", qx(-curve(t, &[(0., 20.), (0.08, 28.), (0.2, -6.), (0.4, 0.)])));
            let arm = curve(t, &[(0., -40.), (0.08, -50.), (0.2, 160.), (0.4, 170.)]);
            for side in 0..2 { p.arm(side, arm, 18., 10.); p.fingers(side, 0.2, 0.1, 0.1); }
            let air = smooth01(0.18, 0.3, t);
            for side in 0..2 {
                let x = super::body::sfx(side); let sg = side_sign(side);
                let a = rig.rest[j(&format!("foot_{x}")) as usize];
                let hips = p.pos("hips");
                let hang = [a[0], hips[1] - (rig.hips_y - a[1]) - 0.02 * u, a[2] + 0.04 * u];
                p.foot(side, lerp(a, hang, air), -30. * air, 20. * air, sg * 5.);
            }
            p.look(0., -20. * air); p.mouth(0.4 * air, 0.3); p.brows(0.6 * air, 0.);
        }),
        "vault" => sample(name, rig, 0.35, 30., false, &[], &|t, p| {
            let k = t / 0.35;
            let sweep = curve(k, &[(0., 0.), (0.4, 1.), (1., 0.2)]);
            // Lean over the planted left hand while the legs swing out to the right.
            p.shift([0.06 * u * sweep, 0.16 * u * (PI * k).sin(), 0.]);
            p.rot("hips", qmul(qz(24. * sweep), qy(-15. * sweep)));
            p.hand(0, [-0.22 * u, rig.hips_y - 0.12 * u, -0.25 * u], [-1., -0.5, 0.], Some(([0.2, 0., -1.], [0., -1., 0.])));
            p.arm(1, 60., 60., 20.);
            for side in 0..2 { let x = super::body::sfx(side); p.set(&format!("upper_leg_{x}"), qmul(qz(-75. * sweep), qx(15. * sweep))); p.set(&format!("lower_leg_{x}"), qx(-45. * sweep)); }
            p.look(-10. * sweep, 10.);
        }),
        "land_roll" => sample(name, rig, 0.5, 30., false, &[], &|t, p| {
            let k = t / 0.5;
            let spin = curve(k, &[(0., 0.), (0.15, 30.), (0.7, 330.), (1., 360.)]);
            let tuck = smooth01(0.05, 0.25, k) * (1. - smooth01(0.7, 0.95, k));
            p.set("hips", qmul(qz(10. * tuck), qx(-spin)));
            p.shift([0., -0.45 * rig.leg_len * tuck, -0.3 * u * k]);
            p.rot("spine", qx(-30. * tuck)); p.rot("chest", qx(-20. * tuck));
            for side in 0..2 { p.arm(side, 40. + 50. * tuck, 30., 60. * tuck + 20.); }
            for side in 0..2 { let x = super::body::sfx(side); p.set(&format!("upper_leg_{x}"), qx(110. * tuck)); p.set(&format!("lower_leg_{x}"), qx(-130. * tuck)); }
            p.look(0., 25. * tuck);
        }),
        "drive" | "sit" => {
            let drive = name == "drive";
            sample(name, rig, 2.0, 10., true, &[], &move |t, p| {
                let s = (TAU * t / 2.).sin();
                let seat = 0.44 * rig.leg_len;
                p.shift([0., -(rig.hips_y - rig.ankle_y) + seat + 0.05 * u, 0.06 * u]);
                p.rot("hips", qx(-6.)); p.rot("spine", qx(8.)); p.rot("chest", qx(4.));
                for side in 0..2 {
                    let x = super::body::sfx(side); let sg = side_sign(side);
                    let hip = p.pos(&format!("upper_leg_{x}"));
                    let foot = [hip[0] + sg * 0.02 * u, rig.ankle_y + 0.02 * u + if drive { 0.08 * u } else { 0. }, hip[2] - if drive { 0.62 } else { 0.42 } * rig.leg_len];
                    p.foot(side, foot, if drive { 20. } else { 0. }, 0., sg * 6.);
                }
                if drive {
                    let wheel = [0., rig.sh - 0.18 * u, -0.42 * u];
                    for side in 0..2 {
                        let sg = side_sign(side);
                        let a = (sg * (70. + 10. * s)).to_radians();
                        p.hand(side, add(wheel, [a.sin() * 0.17 * u, a.cos() * 0.17 * u, 0.]), [sg, -1., 0.3], Some((norm([-sg * 0.3, 0.2, -1.]), [-sg, 0., 0.])));
                        p.fingers(side, 0.9, 0.85, 0.6);
                    }
                    p.look(4. * s, 4.);
                } else {
                    for side in 0..2 { p.arm(side, 40., 12., 60.); p.fingers(side, 0.35, 0.3, 0.2); }
                    p.look(6. * s, 0.);
                }
                p.blink(curve(t, &[(1.0, 0.), (1.08, 1.), (1.16, 0.)]));
            })
        }
        _ => return None,
    })
}
fn walk_clone(g: &Gait) -> Gait { Gait { ..*g } }
impl Clone for Gait { fn clone(&self) -> Self { *self } }
impl Copy for Gait {}

const HOLD_FORCE: [&str; 30] = ["chest", "neck", "head", "shoulder_l", "upper_arm_l", "lower_arm_l", "hand_l", "shoulder_r", "upper_arm_r", "lower_arm_r", "hand_r",
    "thumb_1_l", "thumb_2_l", "index_1_l", "index_2_l", "fingers_1_l", "fingers_2_l", "thumb_1_r", "thumb_2_r", "index_1_r", "index_2_r", "fingers_1_r", "fingers_2_r",
    "spine", "jaw", "lid_l", "lid_r", "brow_l", "brow_r", "mouth_l"];

/// Where the right hand holds a rifle grip in the neutral aim pose, and
/// the hand orientation (finger direction, palm normal) for it.
fn grip_target(rig: &RigInfo) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let u = rig.u;
    let pos = [0.1 * u, rig.sh - 0.1 * u, -0.24 * u];
    (pos, norm([0.05, -0.35, -1.]), [-1., 0., 0.])
}

/// Rifle held at the shoulder, aimed forward; `pitch` -1..1 aims down/up,
/// `kick` is recoil.
fn hold_pose(p: &mut Pose, pitch: f64, kick: f64) {
    let rig = p.rig;
    let u = rig.u;
    // Upper body squares up behind the gun; aim pitch bends spine and chest.
    p.rot("spine", qmul(qy(-12.), qx(pitch * 16.)));
    p.rot("chest", qmul(qy(-10.), qx(pitch * 22. + kick * 4.)));
    p.rot("neck", qmul(qy(14.), qx(pitch * 6.)));
    p.rot("head", qmul(qy(8.), qmul(qz(-6.), qx(pitch * 4. - kick * 3.))));
    // Hand targets move with the chest.
    let chest = p.grot("chest");
    let chest_pos = p.pos("chest");
    let rest_chest = rig.rest[j("chest") as usize];
    let (g, d, n) = grip_target(rig);
    let rel = |v: [f64; 3]| add(chest_pos, qrot(chest, sub(v, rest_chest)));
    let kick_off = [0., 0.012 * u * kick, 0.03 * u * kick];
    let grip = rel(add(g, kick_off));
    let fwd = qrot(chest, norm([0., 0.06 * kick, -1.]));
    p.hand(1, grip, qrot(chest, [1., -1.2, 0.2]), Some((qrot(chest, d), qrot(chest, n))));
    p.fingers(1, 0.95, 0.55, 0.7);
    let fore = add(grip, add(mul(fwd, 0.3 * u), qrot(chest, [-0.03 * u, 0.025 * u, 0.])));
    p.hand(0, fore, qrot(chest, [-0.6, -1., 0.]), Some((qrot(chest, norm([0.75, 0.25, -0.6])), qrot(chest, norm([0.1, 1., 0.1])))));
    p.fingers(0, 0.75, 0.7, 0.5);
    p.brows(0., 0.35); p.mouth(0., -0.05);
    p.blink(0.15);
}

fn reload_pose(p: &mut Pose, t: f64) {
    let rig = p.rig;
    let u = rig.u;
    // Gun tilts, left hand drops the magazine, fetches a new one, seats it.
    let tilt = curve(t, &[(0., 0.), (0.25, 1.), (1.8, 1.), (2.2, 0.)]);
    p.rot("spine", qmul(qy(-12.), qx(-6. * tilt)));
    p.rot("chest", qmul(qy(-10. + 8. * tilt), qx(-8. * tilt)));
    p.rot("neck", qmul(qy(14. - 10. * tilt), qx(12. * tilt)));
    p.rot("head", qx(10. * tilt));
    let chest = p.grot("chest");
    let chest_pos = p.pos("chest");
    let rest_chest = rig.rest[j("chest") as usize];
    let (g, d, n) = grip_target(rig);
    let rel = |v: [f64; 3]| add(chest_pos, qrot(chest, sub(v, rest_chest)));
    let grip = rel(add(g, [-0.05 * u * tilt, -0.06 * u * tilt, 0.04 * u * tilt]));
    let roll = qaxis(qrot(chest, [0., 0., -1.]), -35. * tilt);
    p.hand(1, grip, qrot(chest, [1., -1.2, 0.2]), Some((qrot(roll, qrot(chest, d)), qrot(roll, qrot(chest, n)))));
    p.fingers(1, 0.95, 0.55, 0.7);
    let mag = add(grip, qrot(chest, [-0.01 * u, -0.06 * u, -0.1 * u]));
    let fore = add(grip, qrot(chest, [-0.03 * u, 0.025 * u, -0.3 * u]));
    let pouch = add(rel([-0.1 * u, rig.sh - 0.42 * u, -0.14 * u]), [0., 0., 0.]);
    let keys = |k: usize| -> [f64; 3] { [fore, mag, add(mag, [0., -0.12 * u, 0.02 * u]), pouch, pouch, mag, mag, fore][k] };
    let times = [0., 0.25, 0.5, 0.8, 1.05, 1.4, 1.6, 2.0];
    let i = (0..times.len() - 1).find(|&i| t < times[i + 1]).unwrap_or(times.len() - 2);
    let s = ease(((t - times[i]) / (times[i + 1] - times[i])).clamp(0., 1.));
    let target = lerp(keys(i), keys(i + 1), s);
    // Slap the magazine home.
    let slap = curve(t, &[(1.4, 0.), (1.5, 0.03), (1.6, 0.)]);
    let target = add(target, qrot(chest, [0., slap * u, 0.]));
    p.hand(0, target, qrot(chest, [-0.6, -1., 0.]), Some((qrot(chest, norm([0.6, 0.4, -0.6])), qrot(chest, norm([0.2, 1., 0.2])))));
    p.fingers(0, curve(t, &[(0., 0.75), (0.3, 0.5), (0.5, 0.9), (1.05, 0.9), (1.6, 0.6), (2.2, 0.75)]), 0.7, 0.5);
    p.look(0., 0.); p.brows(0.1, 0.2);
}

fn death_pose(p: &mut Pose, t: f64, back: bool) {
    let rig = p.rig;
    let u = rig.u;
    let s = if back { 1. } else { -1. };
    // Knees buckle, then the body tips over and lands with a small bounce.
    let buckle = curve(t, &[(0., 0.), (0.25, 0.35), (0.45, 0.55), (0.8, 0.85), (1.5, 0.85)]);
    let fall = curve(t, &[(0., 0.), (0.2, 0.05), (0.55, 0.55), (0.75, 1.0), (0.85, 0.96), (1.0, 1.0), (1.5, 1.0)]);
    let ground = (rig.hips_y - 0.09 * u) * fall;
    p.shift([0., -buckle * 0.35 * rig.leg_len * (1. - fall) - ground, s * 0.25 * u * fall]);
    p.set("hips", qmul(qx(s * 82. * fall), qz(6. * fall)));
    p.rot("spine", qx(s * 6. * fall - 10. * (1. - fall) * buckle));
    p.rot("chest", qmul(qx(s * 4. * fall), qy(8. * fall)));
    p.look(18. * fall, s * 10. * fall - 20. * buckle * (1. - fall));
    for side in 0..2 {
        let sg = side_sign(side);
        let flop = curve(t, &[(0., 0.), (0.3, 30.), (0.7, 70.), (1.5, 80.)]);
        p.arm(side, s * 20. * (1. - fall) + 20. * fall * sg, mix(12., 60. + 20. * sg, fall), 20. + flop * 0.3);
        p.fingers(side, 0.4, 0.3, 0.2);
    }
    for side in 0..2 {
        let x = super::body::sfx(side);
        let knee = buckle * 70. * (1. - fall) + 20. * fall * if side == 0 { 1. } else { 0.3 };
        p.set(&format!("upper_leg_{x}"), qx(knee * 0.9 * s.max(0.)));
        p.set(&format!("lower_leg_{x}"), qx(-knee));
        p.set(&format!("foot_{x}"), qx(-20. * fall));
    }
    p.blink(curve(t, &[(0., 0.), (0.1, 0.8), (0.4, 0.5), (0.9, 0.9), (1.5, 0.92)]));
    p.mouth(curve(t, &[(0., 0.), (0.1, 0.6), (0.6, 0.3), (1.5, 0.25)]), -0.3);
    p.brows(0.2, 0.7 * (1. - fall));
}

fn fight_stance(p: &mut Pose, t: f64, block: f64) {
    let rig = p.rig;
    let u = rig.u;
    let bounce = (TAU * t).sin();
    p.shift([0., -0.07 * rig.leg_len - 0.012 * u * (bounce * 0.5 + 0.5), 0.]);
    p.rot("hips", qy(-28.));
    p.rot("spine", qmul(qy(12.), qx(-6.)));
    p.rot("chest", qmul(qy(8.), qx(-4. - 2. * block)));
    p.look(10., 10. + 6. * block);
    // Fists up guarding the chin.
    let chest = p.grot("chest");
    let cp = p.pos("chest"); let rc = rig.rest[j("chest") as usize];
    let rel = |v: [f64; 3]| add(cp, qrot(chest, sub(v, rc)));
    let chin = rig.sh + 0.02 * u;
    let lead = rel([-0.06 * u, mix(chin - 0.02 * u, chin + 0.07 * u, block), mix(-0.3, -0.2, block) * u]);
    let rear = rel([0.1 * u, mix(chin - 0.05 * u, chin + 0.06 * u, block), mix(-0.16, -0.17, block) * u]);
    p.hand(0, add(lead, [0., 0.005 * u * bounce, 0.]), qrot(chest, [-1., -1., 0.3]), Some((qrot(chest, norm([0.2, 0.9, -0.4])), qrot(chest, [0.9, 0., -0.3]))));
    p.hand(1, add(rear, [0., -0.005 * u * bounce, 0.]), qrot(chest, [1., -1., 0.3]), Some((qrot(chest, norm([-0.2, 0.9, -0.4])), qrot(chest, [-0.9, 0., -0.3]))));
    // Open guard: fingers loosely curled, not fists.
    for side in 0..2 { p.fingers(side, 0.35, 0.3, 0.3); }
    // Feet: lead foot forward, rear foot back, turned.
    for side in 0..2 {
        let x = super::body::sfx(side); let sg = side_sign(side);
        let a = rig.rest[j(&format!("foot_{x}")) as usize];
        let z = if side == 0 { -0.13 * u } else { 0.12 * u };
        p.foot(side, [a[0] + sg * 0.05 * u, a[1] + if side == 1 { 0.01 * u * (bounce * 0.5 + 0.5) } else { 0. }, a[2] + z], if side == 1 { -8. } else { 0. }, if side == 1 { 8. } else { 0. }, -24. + sg * 6.);
    }
    p.brows(-0.1, 0.8); p.mouth(0., -0.35);
}

fn punch_pose(p: &mut Pose, t: f64, right: bool) {
    let rig = p.rig;
    let u = rig.u;
    let d = if right { 0.5 } else { 0.36 };
    let k = t / d;
    fight_stance(p, 0., 0.);
    // Anticipation, snap, hold, recover.
    let ext = curve(k, &[(0., 0.), (0.2, -0.15), (0.42, 1.), (0.6, 1.), (1., 0.)]);
    let twist = if right { 1. } else { -0.4 };
    p.rot("hips", qy(-ext * 22. * twist));
    p.rot("chest", qy(-ext * 26. * twist));
    let side = if right { 1 } else { 0 };
    let chest = p.grot("chest");
    let sh = p.pos(if right { "upper_arm_r" } else { "upper_arm_l" });
    let fwd = qrot(chest, [0., 0., -1.]);
    let guard = p.pos(if right { "hand_r" } else { "hand_l" });
    let reach = 0.56 * u;
    let target = lerp(guard, add(add(sh, mul(fwd, reach)), [0., 0.02 * u, 0.]), ext.max(0.));
    let target = if ext < 0. { add(guard, mul(fwd, ext * 0.4 * reach)) } else { target };
    let sg = side_sign(side);
    p.hand(side, target, qrot(chest, [sg, -0.6, 0.]), Some((fwd, qrot(chest, [0., -1., 0.]))));
    p.fingers(side, 1., 1., 0.9);
    p.look(-10. * ext * twist, 8.);
    p.brows(-0.5, 1.4); p.mouth(1.3 * ext, -0.9);
}

fn kick_pose(p: &mut Pose, t: f64) {
    let rig = p.rig;
    let u = rig.u;
    fight_stance(p, 0., 0.);
    let k = t / 0.7;
    let chamber = curve(k, &[(0., 0.), (0.25, 1.), (0.45, 1.), (0.7, 0.8), (1., 0.)]);
    let ext = curve(k, &[(0., 0.), (0.3, 0.), (0.45, 1.), (0.62, 1.), (0.8, 0.), (1., 0.)]);
    p.rot("hips", qmul(qy(20. * ext), qz(12. * chamber)));
    p.rot("spine", qz(-10. * chamber));
    p.rot("chest", qx(8. * ext));
    let hip = p.pos("upper_leg_r");
    let knee_up = add(hip, [0.04 * u, -0.12 * u, -0.34 * u]);
    let ankle_chamber = add(hip, [0.08 * u, -0.34 * u, -0.1 * u]);
    let ankle_ext = add(hip, [0.06 * u, 0.02 * u, -0.8 * rig.leg_len]);
    let a = rig.rest[j("foot_r") as usize];
    let target = lerp(lerp(a, ankle_chamber, chamber), ankle_ext, ext);
    let _ = knee_up;
    p.ik("upper_leg_r", "lower_leg_r", "foot_r", target, [0.3, 0.5, -1.]);
    p.set_global("foot_r", qx(-50. * ext - 20. * chamber));
    p.brows(-0.2, 1.); p.mouth(0.3 * ext, -0.5);
}

/// Grip socket for a hand: position in the palm and a rotation (relative to
/// the hand joint's rest frame) whose +X is the barrel and +Y up in the
/// `holding-right` pose.
pub fn grip_socket(rig: &RigInfo, side: usize) -> ([f64; 3], [f64; 4]) {
    let [d, n, _] = rig.hand_frame[side];
    let x = super::body::sfx(side);
    let wrist = rig.rest[j(&format!("hand_{x}")) as usize];
    let pos = add(add(wrist, mul(d, rig.hand_len * 0.4)), mul(n, rig.palm_w * 0.32));
    let mut p = Pose::new(rig);
    hold_pose(&mut p, 0., 0.);
    let h = p.grot(&format!("hand_{x}"));
    // Socket global in the aim pose: +X forward (-Z), +Y up.
    let target = qy(90.);
    (pos, qmul(qinv(h), target))
}
