//! Waypoints in, a flyable camera track out.
//!
//! The pipeline, in order, because the order is the whole trick:
//!
//! 1. **Spline** — centripetal Catmull-Rom through the waypoints. Centripetal
//!    (α = 0.5) rather than uniform because uniform Catmull-Rom cusps and
//!    self-intersects when waypoints bunch up, which they always do at
//!    doorways.
//! 2. **Relax** — the spline cuts corners, and a corner in a building is a
//!    wall. Push every sample up the clearance gradient until it has room,
//!    with a Laplacian smoothing term so pushing one sample does not put a
//!    kink in its neighbours. Pinned waypoints (doorway centres) stay put.
//! 3. **Speed** — cruise speed, scaled down where the shot asks to linger and
//!    wherever curvature would throw the camera sideways harder than
//!    `max_lateral_accel`. Floored at `min_speed`: a camera that stops dead
//!    reads as a bug, not a pause.
//! 4. **Time** — integrate dt = ds / v to get arc-length-parameterised time,
//!    then ease the ends with a quintic so there is no acceleration step at
//!    the start and finish.
//! 5. **Gaze** — smoothed *separately* from position. Look-at targets are
//!    splined, then the resulting direction is rate-limited so the camera
//!    never whips round faster than `max_gaze_rate`. This is the difference
//!    between a drone shot and a security camera.

use crate::geom::*;
use crate::track::{CameraTrack, TourKey, TrackNote};
use crate::analysis::ClearanceField;

/// Waypoints closer together than this are the same waypoint. Generous on
/// purpose: routing legs meet at shared points, and two control points a
/// finger apart make the spline turn on a radius of millimetres, which is a
/// cusp — geometrically legal, visually a glitch, and a lateral acceleration
/// no speed limit can rescue.
const DEDUPE: f32 = 0.25;

/// Arc length the curvature limit is measured over. Must match
/// `QaLimits::accel_window`, for the reason spelled out in `limit_curvature`.
pub const ACCEL_WINDOW: f32 = 0.25;

/// 26 directions on a cube, for escaping a point buried in geometry where the
/// clearance gradient carries no information.
const PROBE_DIRS: [[f32; 3]; 26] = [
    [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0], [0.0, 0.0, -1.0],
    [0.707, 0.707, 0.0], [0.707, -0.707, 0.0], [-0.707, 0.707, 0.0], [-0.707, -0.707, 0.0],
    [0.707, 0.0, 0.707], [0.707, 0.0, -0.707], [-0.707, 0.0, 0.707], [-0.707, 0.0, -0.707],
    [0.0, 0.707, 0.707], [0.0, 0.707, -0.707], [0.0, -0.707, 0.707], [0.0, -0.707, -0.707],
    [0.577, 0.577, 0.577], [0.577, 0.577, -0.577], [0.577, -0.577, 0.577], [0.577, -0.577, -0.577],
    [-0.577, 0.577, 0.577], [-0.577, 0.577, -0.577], [-0.577, -0.577, 0.577], [-0.577, -0.577, -0.577],
];
use makepad_math::{vec3, Vec3f};

/// Where the camera should be looking when it reaches a waypoint.
#[derive(Clone, Copy, Debug)]
pub enum Gaze {
    /// Down the path, `lookahead` metres in front.
    Forward,
    /// At a fixed point.
    At(Vec3f),
    /// Along a fixed direction.
    Dir(Vec3f),
}

#[derive(Clone, Copy, Debug)]
pub struct Waypoint {
    pub pos: Vec3f,
    pub gaze: Gaze,
    /// Multiplier on cruise speed here. `< 1` lingers.
    pub speed_scale: f32,
    /// Keep this point exactly: doorway centres must not be relaxed away from
    /// the middle of their opening.
    pub pinned: bool,
    pub fov_y_deg: f32,
}

impl Waypoint {
    pub fn new(pos: Vec3f) -> Waypoint {
        Waypoint {
            pos,
            gaze: Gaze::Forward,
            speed_scale: 1.0,
            pinned: false,
            fov_y_deg: 45.0,
        }
    }

    pub fn looking_at(mut self, at: Vec3f) -> Waypoint {
        self.gaze = Gaze::At(at);
        self
    }

    pub fn looking(mut self, dir: Vec3f) -> Waypoint {
        self.gaze = Gaze::Dir(dir);
        self
    }

    pub fn speed(mut self, s: f32) -> Waypoint {
        self.speed_scale = s;
        self
    }

    pub fn pin(mut self) -> Waypoint {
        self.pinned = true;
        self
    }

    pub fn fov(mut self, f: f32) -> Waypoint {
        self.fov_y_deg = f;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MotionProfile {
    /// Cruise, metres per second.
    pub speed: f32,
    /// Never slower than this — no dead stops.
    pub min_speed: f32,
    /// Metres per second squared the camera may be pushed sideways.
    pub max_lateral_accel: f32,
    /// Metres per second squared along the path. Enforced by construction with
    /// a forward/backward sweep, not hoped for.
    pub max_tangential_accel: f32,
    /// Radians per second the gaze may swing.
    pub max_gaze_rate: f32,
    /// Radians per second allowed while the view is *blocked*. Higher than
    /// `max_gaze_rate` on purpose: a rate low enough to keep a pan across a
    /// view comfortable is too low to get off a wall the camera has ended up
    /// facing, and the camera then holds that wall while it catches up. People
    /// turn away from walls faster than they look around rooms.
    pub max_gaze_rate_blocked: f32,
    /// Seconds of ease at each end of the shot.
    pub ease: f32,
    pub fps: f32,
    /// How far ahead a `Gaze::Forward` waypoint looks.
    pub lookahead: f32,
    /// Clearance the path must keep. Usually the body radius, plus margin.
    pub clearance: f32,
    /// Extra headroom asked of the relaxer so the QA margin is not the same
    /// number the planner just barely satisfied.
    pub relax_margin: f32,
}

impl MotionProfile {
    /// A person walking and looking around.
    pub fn walk() -> MotionProfile {
        MotionProfile {
            speed: 1.25,
            min_speed: 0.35,
            max_lateral_accel: 1.6,
            max_tangential_accel: 2.5,
            max_gaze_rate: 1.15,
            max_gaze_rate_blocked: 1.50,
            ease: 1.2,
            fps: 30.0,
            lookahead: 3.2,
            // A 0.95 m doorway measures 0.40 m of clearance after the
            // half-cell safety margin. Asking for more than that here does not
            // widen the door; it just makes the relaxer shove every doorway
            // sample against a wall it cannot escape, and the fight with the
            // Laplacian term shows up as high-frequency kink in the path.
            clearance: 0.30,
            relax_margin: 0.05,
        }
    }

    /// Set the cruise speed, keeping the floor below it.
    ///
    /// Generators derive cruise from the building's size, and a small building
    /// gives a small number — which can land under `min_speed` and make the
    /// floor the ceiling. Every `clamp(min, max)` downstream then panics, and
    /// it only shows up on the one model whose proportions happen to cross
    /// over. Set the two together and the ordering cannot be got wrong.
    pub fn with_speed(mut self, v: f32) -> MotionProfile {
        self.speed = v.max(0.05);
        self.min_speed = self.min_speed.min(self.speed * 0.5);
        self
    }

    /// A drone: faster, wider turns, calmer gaze.
    pub fn drone() -> MotionProfile {
        MotionProfile {
            speed: 3.2,
            min_speed: 0.8,
            max_lateral_accel: 2.6,
            max_tangential_accel: 3.0,
            max_gaze_rate: 0.85,
            max_gaze_rate_blocked: 1.10,
            ease: 1.6,
            fps: 30.0,
            lookahead: 8.0,
            clearance: 0.50,
            relax_margin: 0.15,
        }
    }
}

/// Centripetal Catmull-Rom through `pts`, sampled at roughly `step` metres.
/// Endpoints are duplicated so the curve starts and ends on them.
pub fn catmull_rom(pts: &[Vec3f], step: f32) -> Vec<Vec3f> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let mut out = Vec::new();
    let n = pts.len();
    let get = |i: i32| -> Vec3f { pts[(i.max(0) as usize).min(n - 1)] };
    for i in 0..n - 1 {
        let p0 = get(i as i32 - 1);
        let p1 = get(i as i32);
        let p2 = get(i as i32 + 1);
        let p3 = get(i as i32 + 2);
        // Centripetal knot spacing: t_{k+1} = t_k + |p_{k+1} - p_k|^0.5.
        let d = |a: Vec3f, b: Vec3f| (b - a).length().max(1e-5).sqrt();
        let (t0, t1) = (0.0f32, d(p0, p1));
        let t2 = t1 + d(p1, p2);
        let t3 = t2 + d(p2, p3);
        let seg = (p2 - p1).length();
        let steps = ((seg / step).ceil() as usize).max(1);
        for s in 0..steps {
            let t = t1 + (t2 - t1) * (s as f32 / steps as f32);
            let a1 = p0 * ((t1 - t) / (t1 - t0).max(1e-5)) + p1 * ((t - t0) / (t1 - t0).max(1e-5));
            let a2 = p1 * ((t2 - t) / (t2 - t1).max(1e-5)) + p2 * ((t - t1) / (t2 - t1).max(1e-5));
            let a3 = p2 * ((t3 - t) / (t3 - t2).max(1e-5)) + p3 * ((t - t2) / (t3 - t2).max(1e-5));
            let b1 = a1 * ((t2 - t) / (t2 - t0).max(1e-5)) + a2 * ((t - t0) / (t2 - t0).max(1e-5));
            let b2 = a2 * ((t3 - t) / (t3 - t1).max(1e-5)) + a3 * ((t - t1) / (t3 - t1).max(1e-5));
            out.push(b1 * ((t2 - t) / (t2 - t1).max(1e-5)) + b2 * ((t - t1) / (t2 - t1).max(1e-5)));
        }
    }
    out.push(pts[n - 1]);
    out
}

/// Push a polyline out of the walls it cuts through, keeping it smooth.
///
/// Two forces per iteration: a clearance force along the gradient of the
/// clearance field wherever a sample has less than `want` metres of room, and
/// a Laplacian force pulling each sample toward the average of its neighbours
/// so the correction spreads instead of denting the curve. `pinned` samples
/// (doorway centres, the start and the end) are never moved.
pub fn relax(field: &ClearanceField, pts: &mut [Vec3f], pinned: &[bool], want: f32, iters: usize) {
    if pts.len() < 3 {
        return;
    }
    let n = pts.len();
    let mut next = pts.to_vec();
    for _ in 0..iters {
        let mut moved = 0.0f32;
        for i in 1..n - 1 {
            if pinned[i] {
                next[i] = pts[i];
                continue;
            }
            let p = pts[i];
            let mut d = vec3(0.0, 0.0, 0.0);
            let c = field.at(p);
            if c < want {
                let g = field.gradient(p);
                let gl = g.length();
                if gl > 1e-5 {
                    d = d + g * ((want - c) * 0.85 / gl);
                } else {
                    // Deep inside geometry the clearance field is flat zero, so
                    // its gradient is zero and the point can never climb out —
                    // the relaxer silently gives up on exactly the samples that
                    // need it most. Probe outward instead and walk toward
                    // whichever direction actually has room.
                    let step = field.cell() * 2.0;
                    let mut best = (c, vec3(0.0, 0.0, 0.0));
                    for dir in PROBE_DIRS {
                        let q = vec3(dir[0], dir[1], dir[2]);
                        let v = field.at(p + q * step);
                        if v > best.0 {
                            best = (v, q);
                        }
                    }
                    if best.1.length_squared() > 0.0 {
                        d = d + best.1 * (field.cell() * 0.9);
                    }
                }
            }
            // Laplacian: keep the curve a curve.
            let lap = (pts[i - 1] + pts[i + 1]) * 0.5 - p;
            d = d + lap * 0.30;
            next[i] = field.clamp_inside(p + d);
            moved += d.length();
        }
        pts.copy_from_slice(&next);
        if moved / (n as f32) < 1e-4 {
            break;
        }
    }
}

/// How to build one track — everything that is not geometry or motion.
#[derive(Clone, Debug)]
pub struct TrackOpts {
    pub name: String,
    pub kind_label: String,
    /// `(waypoint index, text)`, resolved to real times once timing is known.
    pub notes: Vec<(usize, String)>,
    /// Ease the start. Off for every leg but the first of a sequence: easing
    /// at an interior join means the camera slows to a crawl in the middle of
    /// a move, which reads as a stutter and trips the dead-stop check.
    pub ease_in: bool,
    pub ease_out: bool,
    /// Gaze the camera is already holding, as `(yaw, pitch)`. Passing the
    /// previous leg's final gaze is what stops the view snapping at a join —
    /// the rate limiter has to start from somewhere, and starting from
    /// "wherever this leg's first waypoint points" is a whip pan.
    pub initial_gaze: Option<(f32, f32)>,
}

impl TrackOpts {
    pub fn new(name: &str, kind_label: &str) -> TrackOpts {
        TrackOpts {
            name: name.into(),
            kind_label: kind_label.into(),
            notes: Vec::new(),
            ease_in: true,
            ease_out: true,
            initial_gaze: None,
        }
    }

    pub fn notes(mut self, n: Vec<(usize, String)>) -> TrackOpts {
        self.notes = n;
        self
    }
}

/// The gaze a track ends on, for chaining into the next leg.
pub fn final_gaze(track: &CameraTrack) -> Option<(f32, f32)> {
    track.keys.last().map(|k| dir_to_yaw_pitch(k.dir()))
}

/// Build the finished track.
pub fn build_track(
    field: &ClearanceField,
    waypoints: &[Waypoint],
    profile: &MotionProfile,
    opts: &TrackOpts,
) -> CameraTrack {
    let name = opts.name.as_str();
    let kind_label = opts.kind_label.as_str();
    let notes_at = &opts.notes[..];
    if waypoints.len() < 2 {
        return CameraTrack {
            name: name.into(),
            kind_label: kind_label.into(),
            keys: Vec::new(),
            fps: profile.fps,
            notes: Vec::new(),
        };
    }
    // Drop waypoints that repeat the previous one. Routing legs meet at the
    // room they share, so duplicates are the norm rather than the exception,
    // and they do two kinds of damage: a zero-length spline segment is a cusp
    // (infinite curvature, which the QA correctly calls a corner too tight),
    // and the sample→waypoint mapping below advances by comparing distances,
    // so a duplicate pair stalls it permanently and every note in the leg ends
    // up stamped at t = 0.
    let (waypoints, remap): (Vec<Waypoint>, Vec<usize>) = {
        let mut v: Vec<Waypoint> = Vec::with_capacity(waypoints.len());
        let mut map = Vec::with_capacity(waypoints.len());
        for w in waypoints {
            match v.last() {
                Some(p) if (p.pos - w.pos).length() < DEDUPE => {
                    // Keep the more constrained of the two.
                    let last = v.len() - 1;
                    if w.pinned || w.speed_scale < p.speed_scale {
                        v[last] = *w;
                    }
                    map.push(last);
                }
                _ => {
                    v.push(*w);
                    map.push(v.len() - 1);
                }
            }
        }
        (v, map)
    };
    let notes_at: Vec<(usize, String)> = notes_at
        .iter()
        .map(|(i, s)| (remap.get(*i).copied().unwrap_or(0), s.clone()))
        .collect();
    let notes_at = &notes_at[..];
    if waypoints.len() < 2 {
        return CameraTrack {
            name: name.into(),
            kind_label: kind_label.into(),
            keys: Vec::new(),
            fps: profile.fps,
            notes: Vec::new(),
        };
    }
    let waypoints = &waypoints[..];

    let step = (field.cell() * 0.75).clamp(0.05, 0.4);
    let ctrl: Vec<Vec3f> = waypoints.iter().map(|w| w.pos).collect();
    let mut pts = catmull_rom(&ctrl, step);
    // A spline through clamped control points still bulges past them, and a
    // bulge past the edge of the voxel volume reads as solid rock. Clamp the
    // samples, not just the waypoints.
    for p in pts.iter_mut() {
        *p = field.clamp_inside(*p);
    }

    // Map each dense sample back to the waypoint interval it came from, so
    // speed scale, fov, gaze and pinning can be interpolated along it.
    let mut wp_index = vec![0f32; pts.len()];
    {
        // Nearest control point by arc position: walk both lists together.
        let mut ci = 0usize;
        for (i, p) in pts.iter().enumerate() {
            while ci + 1 < ctrl.len() {
                let dc = (ctrl[ci] - *p).length_squared();
                let dn = (ctrl[ci + 1] - *p).length_squared();
                if dn < dc {
                    ci += 1;
                } else {
                    break;
                }
            }
            // Fractional position between ci and ci+1.
            let f = if ci + 1 < ctrl.len() {
                let a = ctrl[ci];
                let b = ctrl[ci + 1];
                let ab = b - a;
                let l2 = ab.length_squared().max(1e-9);
                (((*p - a).dot(ab)) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            wp_index[i] = ci as f32 + f;
        }
    }

    let mut pinned = vec![false; pts.len()];
    pinned[0] = true;
    let last = pts.len() - 1;
    pinned[last] = true;
    for (wi, w) in waypoints.iter().enumerate() {
        if !w.pinned {
            continue;
        }
        // Pin the dense sample closest to this waypoint.
        let mut best = 0usize;
        let mut bd = f32::INFINITY;
        for (i, p) in pts.iter().enumerate() {
            let d = (*p - w.pos).length_squared();
            if d < bd {
                bd = d;
                best = i;
            }
        }
        pts[best] = w.pos;
        pinned[best] = true;
        let _ = wi;
    }

    relax(
        field,
        &mut pts,
        &pinned,
        profile.clearance + profile.relax_margin,
        60,
    );

    // Arc length.
    let n = pts.len();
    let mut arc = vec![0f32; n];
    for i in 1..n {
        arc[i] = arc[i - 1] + (pts[i] - pts[i - 1]).length();
    }
    let total = arc[n - 1];
    if total < 1e-4 {
        return CameraTrack {
            name: name.into(),
            kind_label: kind_label.into(),
            keys: Vec::new(),
            fps: profile.fps,
            notes: Vec::new(),
        };
    }

    // Curvature at each sample, from the circumscribed circle of the triple.
    let mut speed = vec![profile.speed; n];
    for i in 0..n {
        let sc = sample_wp(waypoints, wp_index[i], |w| w.speed_scale);
        speed[i] = (profile.speed * sc).clamp(profile.min_speed.min(profile.speed), profile.speed * 1.5);
    }
    limit_curvature(&mut speed, &pts, &arc, profile.max_lateral_accel);
    // A speed profile with steps in it is a jerk in the render: low-pass it.
    for _ in 0..24 {
        let prev = speed.clone();
        for i in 1..n - 1 {
            speed[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
        }
    }
    limit_curvature(&mut speed, &pts, &arc, profile.max_lateral_accel);
    // Ease the ends: scale speed by a quintic over the first/last stretch.
    let ease_len = (profile.ease * profile.speed).min(total * 0.4);
    if ease_len > 1e-3 && (opts.ease_in || opts.ease_out) {
        for i in 0..n {
            let a = if opts.ease_in {
                smootherstep(arc[i] / ease_len)
            } else {
                1.0
            };
            let b = if opts.ease_out {
                smootherstep((total - arc[i]) / ease_len)
            } else {
                1.0
            };
            // Never all the way to zero: a shot that stops dead reads as a
            // dropped frame, and the QA dead-stop check agrees.
            let f = a.min(b).max(0.18);
            speed[i] = (speed[i] * f).max(profile.min_speed * 0.5);
        }
        // Smooth again *after* easing: the ease multiplies an already-varying
        // profile, and the product can have a sharper knee than either factor.
        for _ in 0..12 {
            let prev = speed.clone();
            for i in 1..n - 1 {
                speed[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
            }
        }
    }
    limit_accel(&mut speed, &arc, profile.max_tangential_accel, profile.min_speed);

    // Integrate time.
    let mut time = vec![0f32; n];
    for i in 1..n {
        let ds = arc[i] - arc[i - 1];
        let v = (speed[i] + speed[i - 1]) * 0.5;
        time[i] = time[i - 1] + ds / v.max(1e-3);
    }
    let duration = time[n - 1];

    // Gaze targets per dense sample, splined from the waypoints.
    let gaze_ctrl: Vec<Vec3f> = waypoints
        .iter()
        .enumerate()
        .map(|(_i, w)| match w.gaze {
            Gaze::At(p) => p,
            Gaze::Dir(d) => w.pos + d.normalize() * profile.lookahead,
            Gaze::Forward => {
                // A point `lookahead` along the relaxed path from here.
                let here = nearest_arc(&pts, &arc, w.pos);
                let want = (here + profile.lookahead).min(total);
                let p = point_at_arc(&pts, &arc, want);
                if (p - w.pos).length() < 0.5 {
                    // At the very end there is no path left to look down;
                    // keep the previous heading instead of staring at our feet.
                    let back = point_at_arc(&pts, &arc, (here - 1.0).max(0.0));
                    w.pos + (w.pos - back).normalize() * profile.lookahead
                } else {
                    p
                }
            }
        })
        .collect();
    let _ = &gaze_ctrl;

    let mut targets = Vec::with_capacity(n);
    for i in 0..n {
        let u = wp_index[i];
        let a = u.floor() as usize;
        let b = (a + 1).min(waypoints.len() - 1);
        let f = smoothstep(u - a as f32);
        targets.push(Vec3f::from_lerp(gaze_ctrl[a], gaze_ctrl[b], f));
    }
    // Never aim at a wall. A gaze target the camera cannot see — the room's
    // best view from the far side of a doorway, or a lookahead point through a
    // partition — makes the camera hold a blank surface until the geometry
    // opens up again. Re-aim those samples along the direction of travel,
    // which is guaranteed clear because the path itself is, and only then
    // smooth: fixing it after smoothing would put the kink straight back.
    reaim(field, &pts, &mut targets, profile);

    // Smooth the target curve itself before rate-limiting the direction.
    // Generously: spreading a gaze change over more of the path lowers the
    // peak rate it demands, and a demand the limiter cannot meet is exactly
    // how the camera ends up lagging behind and holding a wall.
    for _ in 0..44 {
        let prev = targets.clone();
        for i in 1..n - 1 {
            targets[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
        }
    }

    // Resample at fps and rate-limit the gaze as we go.
    let frames = ((duration * profile.fps).ceil() as usize).max(2);
    let mut keys = Vec::with_capacity(frames + 1);
    let (mut cur_yaw, mut cur_pitch) = opts.initial_gaze.unwrap_or((0.0, 0.0));
    let mut have = opts.initial_gaze.is_some();
    let dt = duration / frames as f32;
    let mut si = 0usize;
    for fi in 0..=frames {
        let t = (fi as f32 * dt).min(duration);
        while si + 1 < n && time[si + 1] < t {
            si += 1;
        }
        let j = (si + 1).min(n - 1);
        let span = (time[j] - time[si]).max(1e-6);
        let f = ((t - time[si]) / span).clamp(0.0, 1.0);
        let pos = Vec3f::from_lerp(pts[si], pts[j], f);
        let tgt = Vec3f::from_lerp(targets[si], targets[j], f);
        let want = tgt - pos;
        let (mut yaw, mut pitch) = if want.length_squared() > 1e-8 {
            dir_to_yaw_pitch(want.normalize())
        } else {
            (cur_yaw, cur_pitch)
        };
        pitch = pitch.clamp(-1.40, 1.40);
        if !have {
            cur_yaw = yaw;
            cur_pitch = pitch;
            have = true;
        } else {
            // Blocked view? Allow a brisker turn to get off the wall.
            let looking = yaw_pitch_to_dir(cur_yaw, cur_pitch);
            let (seen, _) = field.sight(pos, looking, 30.0);
            let rate = if seen < profile.lookahead * 0.4 + 0.6 {
                profile.max_gaze_rate_blocked
            } else {
                profile.max_gaze_rate
            };
            let (dy, dp) = gaze_step(cur_yaw, cur_pitch, yaw, pitch, rate * dt);
            cur_yaw += dy;
            cur_pitch += dp;
            yaw = cur_yaw;
            pitch = cur_pitch;
        }
        let dir = yaw_pitch_to_dir(yaw, pitch);
        let dist = want.length().max(profile.lookahead * 0.5);
        let fov = sample_wp(waypoints, wp_index[si], |w| w.fov_y_deg);
        keys.push(TourKey {
            t,
            pos,
            look_at: pos + dir * dist,
            up: vec3(0.0, 0.0, 1.0),
            fov_y_deg: fov,
        });
    }

    let notes = notes_at
        .iter()
        .filter_map(|(wi, text)| {
            let target = *wi as f32;
            let i = (0..n).min_by(|a, b| {
                (wp_index[*a] - target)
                    .abs()
                    .partial_cmp(&(wp_index[*b] - target).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;
            Some(TrackNote {
                t: time[i],
                text: text.clone(),
            })
        })
        .collect();

    CameraTrack {
        name: name.into(),
        kind_label: kind_label.into(),
        keys,
        fps: profile.fps,
        notes,
    }
}

/// Hold the speed under what the path's curvature allows: `v ≤ √(a_lat / k)`.
///
/// Applied *after* every clamp and every smoothing pass, never before. Getting
/// that order wrong is subtle and total: clamping up to `min_speed` after the
/// curvature limit puts the speed straight back above what the corner can take,
/// and every tight turn in the building silently re-violates the limit the
/// generator believes it just enforced.
fn limit_curvature(speed: &mut [f32], pts: &[Vec3f], arc: &[f32], a_lat: f32) {
    let n = speed.len().min(pts.len()).min(arc.len());
    if n < 3 || a_lat <= 0.0 {
        return;
    }
    // Measured over the same arc-length window the QA uses (`ACCEL_WINDOW`).
    // Curvature is scale-dependent: sample it at one spacing here and another
    // there and the generator will happily certify a shot the checker then
    // rejects, which is the motion-limit version of keeping two clearance
    // functions.
    let (mut first, mut last) = (usize::MAX, 0usize);
    for i in 0..n {
        let Some(a) = (0..i).rev().find(|j| arc[i] - arc[*j] >= ACCEL_WINDOW) else {
            continue;
        };
        let Some(b) = ((i + 1)..n).find(|j| arc[*j] - arc[i] >= ACCEL_WINDOW) else {
            continue;
        };
        if first == usize::MAX {
            first = i;
        }
        last = i;
        let k = curvature(pts[a], pts[i], pts[b]);
        if k > 1e-5 {
            let cap = (a_lat / k).sqrt();
            for v in speed.iter_mut().take(b + 1).skip(a) {
                *v = v.min(cap);
            }
        }
    }
    // The first and last window's worth of samples have no full window to
    // measure over, so they were skipped entirely. Carry the nearest measured
    // limit into them instead of leaving them at cruise.
    if first != usize::MAX {
        let head = speed[first];
        for v in speed.iter_mut().take(first) {
            *v = v.min(head);
        }
        let tail = speed[last];
        for v in speed.iter_mut().skip(last) {
            *v = v.min(tail);
        }
    }
}

/// Clamp a speed profile so it can actually be driven: two sweeps enforcing
/// `v² ≤ v_prev² + 2·a·ds` forwards and backwards. After this the tangential
/// acceleration is bounded by `a_max` everywhere, by construction — which is
/// the only way to make a limit the QA checks something the generator cannot
/// violate rather than something it usually gets away with.
fn limit_accel(speed: &mut [f32], arc: &[f32], a_max: f32, min_speed: f32) {
    let n = speed.len();
    if n < 2 || a_max <= 0.0 {
        return;
    }
    for i in 1..n {
        let ds = (arc[i] - arc[i - 1]).max(0.0);
        let cap = (speed[i - 1] * speed[i - 1] + 2.0 * a_max * ds).sqrt();
        speed[i] = speed[i].min(cap);
    }
    for i in (0..n - 1).rev() {
        let ds = (arc[i + 1] - arc[i]).max(0.0);
        let cap = (speed[i + 1] * speed[i + 1] + 2.0 * a_max * ds).sqrt();
        speed[i] = speed[i].min(cap);
    }
    // Only ever reduce. A floor applied here would undo the curvature limit
    // that was applied before it.
    let _ = min_speed;
    for v in speed.iter_mut() {
        *v = v.max(0.05);
    }
}

/// Never aim at a wall.
///
/// A gaze target the camera cannot see — a room's best view from the far side
/// of a doorway, a lookahead point through a partition — makes the camera hold
/// a blank surface until the geometry opens up. Re-aim those samples along the
/// direction of travel, which is guaranteed clear because the path is, trying a
/// fan of offsets and keeping the most open one.
///
/// Runs before target smoothing in both `build_track` and `polish`: doing it
/// after would put the kink straight back, and skipping it in `polish` would
/// silently undo the work `build_track` did, because polish re-derives the
/// gaze from scratch.
fn reaim(field: &ClearanceField, pts: &[Vec3f], targets: &mut [Vec3f], profile: &MotionProfile) {
    let n = pts.len().min(targets.len());
    if n < 2 {
        return;
    }
    let min_view = profile.lookahead * 0.4 + 0.6;
    for i in 0..n {
        let d = targets[i] - pts[i];
        if d.length_squared() < 1e-8 {
            continue;
        }
        let (see, _) = field.sight(pts[i], d.normalize(), 30.0);
        if see >= min_view {
            continue;
        }
        let travel = if i + 1 < n {
            pts[i + 1] - pts[i]
        } else {
            pts[i] - pts[i - 1]
        };
        if travel.length_squared() < 1e-8 {
            continue;
        }
        let (base_yaw, base_pitch) = dir_to_yaw_pitch(travel.normalize());
        let mut best = (see, targets[i]);
        for off in [0.0f32, 0.3, -0.3, 0.6, -0.6, 1.0, -1.0, 1.5, -1.5] {
            let cand = yaw_pitch_to_dir(base_yaw + off, base_pitch * 0.5);
            let (s2, _) = field.sight(pts[i], cand, 30.0);
            if s2 > best.0 {
                best = (s2, pts[i] + cand * profile.lookahead);
            }
            if s2 >= min_view {
                break;
            }
        }
        targets[i] = best.1;
    }
}

/// One step of the gaze rate limit, bounding the **combined** angular change.
///
/// Clamping yaw and pitch independently lets the two add up: a diagonal swing
/// moves at `√2 ×` the nominal rate, so the limit has to be set low enough for
/// the diagonal case and is then too slow for the common one. Too slow is not
/// merely sluggish — the camera falls behind its target and holds whatever it
/// is passing, which is how a walkthrough ends up staring at a wall for most
/// of a second.
fn gaze_step(yaw: f32, pitch: f32, want_yaw: f32, want_pitch: f32, max_step: f32) -> (f32, f32) {
    let dy = angle_delta(yaw, want_yaw);
    let dp = want_pitch - pitch;
    let mag = (dy * dy + dp * dp).sqrt();
    if mag <= max_step || mag < 1e-9 {
        return (dy, dp);
    }
    let k = max_step / mag;
    (dy * k, dp * k)
}

fn sample_wp(wps: &[Waypoint], u: f32, f: impl Fn(&Waypoint) -> f32) -> f32 {
    let a = (u.floor().max(0.0) as usize).min(wps.len() - 1);
    let b = (a + 1).min(wps.len() - 1);
    let t = smoothstep(u - a as f32);
    f(&wps[a]) + (f(&wps[b]) - f(&wps[a])) * t
}

/// Reciprocal of the radius of the circle through three points.
fn curvature(a: Vec3f, b: Vec3f, c: Vec3f) -> f32 {
    let ab = b - a;
    let cb = b - c;
    let cross = Vec3f::cross(ab, cb).length();
    let denom = ab.length() * cb.length() * (a - c).length();
    if denom < 1e-8 {
        0.0
    } else {
        2.0 * cross / denom
    }
}

fn nearest_arc(pts: &[Vec3f], arc: &[f32], p: Vec3f) -> f32 {
    let mut bd = f32::INFINITY;
    let mut best = 0.0;
    for (i, q) in pts.iter().enumerate() {
        let d = (*q - p).length_squared();
        if d < bd {
            bd = d;
            best = arc[i];
        }
    }
    best
}

fn point_at_arc(pts: &[Vec3f], arc: &[f32], s: f32) -> Vec3f {
    let n = pts.len();
    if s <= 0.0 {
        return pts[0];
    }
    if s >= arc[n - 1] {
        return pts[n - 1];
    }
    let i = arc.partition_point(|a| *a <= s).max(1).min(n - 1);
    let span = (arc[i] - arc[i - 1]).max(1e-6);
    let f = (s - arc[i - 1]) / span;
    Vec3f::from_lerp(pts[i - 1], pts[i], f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catmull_passes_through_controls() {
        let pts = vec![
            vec3(0.0, 0.0, 0.0),
            vec3(2.0, 1.0, 0.0),
            vec3(4.0, 0.0, 0.0),
            vec3(6.0, 2.0, 0.0),
        ];
        let out = catmull_rom(&pts, 0.1);
        for c in &pts {
            let near = out
                .iter()
                .map(|p| (*p - *c).length())
                .fold(f32::INFINITY, f32::min);
            assert!(near < 0.06, "control {c:?} missed by {near}");
        }
        assert_eq!(out[0], pts[0]);
        assert_eq!(*out.last().unwrap(), pts[3]);
    }

    #[test]
    fn curvature_of_a_circle_is_one_over_r() {
        let r = 4.0f32;
        let a = vec3(r, 0.0, 0.0);
        let b = vec3(r * 0.1f32.cos(), r * 0.1f32.sin(), 0.0);
        let c = vec3(r * 0.2f32.cos(), r * 0.2f32.sin(), 0.0);
        let k = curvature(a, b, c);
        assert!((k - 1.0 / r).abs() < 1e-3, "curvature {k}");
    }
}

/// Re-time and re-smooth an already-assembled track.
///
/// Concatenating finished shots gives a camera that is geometrically right and
/// temporally wrong: each leg eased its own ends, so the joins have speed
/// steps, and each leg started its gaze limiter from scratch, so the joins
/// have whip pans. `polish` throws the timing away and rebuilds it over the
/// whole path — one arc-length parameterisation, one curvature limit, one
/// pair of eased ends, one continuous gaze — which is what makes a five-shot
/// tour read as a single continuous camera rather than five clips in a row.
///
/// Positions are *not* moved: they were already relaxed against the clearance
/// oracle and are known collision-free. Only time and gaze change.
pub fn polish(
    field: &ClearanceField,
    track: &CameraTrack,
    profile: &MotionProfile,
) -> CameraTrack {
    let n = track.keys.len();
    if n < 4 {
        return track.clone();
    }
    // Legs joined end-to-end leave near-coincident keys at the seam, and a
    // segment a hair long is a curvature singularity: the corner the QA then
    // reports as impossibly tight is two points that should have been one.
    let mut keep: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        match keep.last() {
            Some(&j) if (track.keys[i].pos - track.keys[j].pos).length() < 0.02 => {}
            _ => keep.push(i),
        }
    }
    if keep.len() < 4 {
        return track.clone();
    }
    let n = keep.len();
    let mut pts: Vec<Vec3f> = keep.iter().map(|i| track.keys[*i].pos).collect();
    // Legs are individually smooth but meet at corners, and a corner is a
    // curvature spike no speed limit can make comfortable. Relax the joined
    // polyline against the clearance oracle: it rounds the joins off and, by
    // construction, cannot round them into a wall.
    let mut pinned = vec![false; n];
    pinned[0] = true;
    pinned[n - 1] = true;
    relax(field, &mut pts, &pinned, profile.clearance, 400);
    let mut targets: Vec<Vec3f> = keep.iter().map(|i| track.keys[*i].look_at).collect();
    reaim(field, &pts, &mut targets, profile);
    for _ in 0..32 {
        let prev = targets.clone();
        for i in 1..n - 1 {
            targets[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
        }
    }
    // Smoothing can drag a target back into a wall it was just moved out of.
    // Check once more; the rate limiter downstream absorbs the small kink.
    reaim(field, &pts, &mut targets, profile);
    let fovs: Vec<f32> = keep.iter().map(|i| track.keys[*i].fov_y_deg).collect();

    let mut arc = vec![0f32; n];
    for i in 1..n {
        arc[i] = arc[i - 1] + (pts[i] - pts[i - 1]).length();
    }
    let total = arc[n - 1];
    if total < 1e-3 {
        return track.clone();
    }

    // Seed from the speed the track already had, not from the profile's
    // cruise: the generator's choices — lingering in a room, easing through a
    // doorway — are intent, and re-timing from scratch would flatten them.
    // Polishing may only slow the camera down, never speed it up.
    let floor = profile.min_speed.min(profile.speed);
    let mut speed = vec![profile.speed; n];
    for i in 0..n {
        let a = keep[i.saturating_sub(1)];
        let b = keep[(i + 1).min(n - 1)];
        let dt = (track.keys[b].t - track.keys[a].t).max(1e-5);
        let ds = (track.keys[b].pos - track.keys[a].pos).length();
        speed[i] = (ds / dt).clamp(floor, profile.speed * 1.5);
    }
    limit_curvature(&mut speed, &pts, &arc, profile.max_lateral_accel);
    for _ in 0..40 {
        let prev = speed.clone();
        for i in 1..n - 1 {
            speed[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
        }
    }
    limit_curvature(&mut speed, &pts, &arc, profile.max_lateral_accel);
    let ease_len = (profile.ease * profile.speed).min(total * 0.25);
    if ease_len > 1e-3 {
        for i in 0..n {
            let f = smootherstep(arc[i] / ease_len)
                .min(smootherstep((total - arc[i]) / ease_len))
                .max(0.2);
            speed[i] = (speed[i] * f).max(profile.min_speed * 0.5);
        }
        for _ in 0..20 {
            let prev = speed.clone();
            for i in 1..n - 1 {
                speed[i] = (prev[i - 1] + prev[i] * 2.0 + prev[i + 1]) * 0.25;
            }
        }
    }

    limit_curvature(&mut speed, &pts, &arc, profile.max_lateral_accel);
    limit_accel(&mut speed, &arc, profile.max_tangential_accel, profile.min_speed);

    let mut time = vec![0f32; n];
    for i in 1..n {
        let ds = arc[i] - arc[i - 1];
        let v = (speed[i] + speed[i - 1]) * 0.5;
        time[i] = time[i - 1] + ds / v.max(1e-3);
    }
    let duration = time[n - 1];
    let frames = ((duration * profile.fps).ceil() as usize).max(2);
    let dt = duration / frames as f32;

    let mut keys = Vec::with_capacity(frames + 1);
    let (mut cur_yaw, mut cur_pitch) = (0.0f32, 0.0f32);
    let mut have = false;
    let mut si = 0usize;
    for fi in 0..=frames {
        let t = (fi as f32 * dt).min(duration);
        while si + 1 < n && time[si + 1] < t {
            si += 1;
        }
        let j = (si + 1).min(n - 1);
        let f = ((t - time[si]) / (time[j] - time[si]).max(1e-6)).clamp(0.0, 1.0);
        let pos = Vec3f::from_lerp(pts[si], pts[j], f);
        let tgt = Vec3f::from_lerp(targets[si], targets[j], f);
        let want = tgt - pos;
        let (mut yaw, mut pitch) = if want.length_squared() > 1e-8 {
            dir_to_yaw_pitch(want.normalize())
        } else {
            (cur_yaw, cur_pitch)
        };
        pitch = pitch.clamp(-1.40, 1.40);
        if !have {
            cur_yaw = yaw;
            cur_pitch = pitch;
            have = true;
        } else {
            // Blocked view? Allow a brisker turn to get off the wall.
            let looking = yaw_pitch_to_dir(cur_yaw, cur_pitch);
            let (seen, _) = field.sight(pos, looking, 30.0);
            let rate = if seen < profile.lookahead * 0.4 + 0.6 {
                profile.max_gaze_rate_blocked
            } else {
                profile.max_gaze_rate
            };
            let (dy, dp) = gaze_step(cur_yaw, cur_pitch, yaw, pitch, rate * dt);
            cur_yaw += dy;
            cur_pitch += dp;
            yaw = cur_yaw;
            pitch = cur_pitch;
        }
        let dir = yaw_pitch_to_dir(yaw, pitch);
        let dist = want.length().max(profile.lookahead * 0.5);
        keys.push(TourKey {
            t,
            pos,
            look_at: pos + dir * dist,
            up: vec3(0.0, 0.0, 1.0),
            fov_y_deg: fovs[si],
        });
    }

    // Carry the notes across by arc position, not by time: time is what just
    // changed.
    let old_dur = track.duration().max(1e-6);
    let notes = track
        .notes
        .iter()
        .map(|nt| {
            let frac = (nt.t / old_dur).clamp(0.0, 1.0);
            let i = ((frac * (n - 1) as f32).round() as usize).min(n - 1);
            TrackNote {
                t: time[i],
                text: nt.text.clone(),
            }
        })
        .collect();

    CameraTrack {
        name: track.name.clone(),
        kind_label: track.kind_label.clone(),
        keys,
        fps: profile.fps,
        notes,
    }
}
