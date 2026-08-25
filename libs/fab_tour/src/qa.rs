//! Fly the track and complain.
//!
//! Every generated shot is flown headless at a fixed step and checked against
//! limits that stand in for a viewer's judgement. The checks fall into three
//! groups:
//!
//! * **Correctness** — the camera is never inside geometry and never so close
//!   to it that the near plane would slice through a wall. This is the one
//!   that must never fail.
//! * **Comfort** — no gaze whip, no cornering that throws the frame sideways,
//!   no dead stops. These are the numbers that decide whether a shot is
//!   watchable, and they are the same family of numbers the Doom walk probe
//!   used (`libs/render/examples/walk_probe.rs`: yaw reversals, straightness,
//!   stuck time).
//! * **Truthfulness** — when the camera leaves one room and appears in
//!   another, it went through a door. A path that slips between two rooms
//!   through a gap in the model is a modelling bug the tour would otherwise
//!   hide.
//!
//! Clearance is read through [`crate::analysis::ClearanceField`], the same
//! oracle the planner used. That is deliberate and load-bearing: a QA that
//! measured clearance its own way would drift from the planner and start
//! failing paths that are fine, or passing paths that are not.

use crate::analysis::{ClearMode, SiteAnalysis};
use crate::geom::angle_between;
use makepad_math::Vec3f as V3;
use crate::track::CameraTrack;
use makepad_math::{vec3, Vec3f};

#[derive(Clone, Copy, Debug)]
pub struct QaLimits {
    /// Metres of geometry-free space the camera must always have. Set low
    /// enough that a 1.6 m eye under a 1.7 m ceiling passes, high enough that
    /// being *in* a wall never does.
    pub min_clearance: f32,
    pub near_plane: f32,
    /// Radians per second the view direction may turn.
    pub max_gaze_rate: f32,
    /// Metres per second squared, perpendicular to travel.
    pub max_lateral_accel: f32,
    /// Metres per second squared, along travel.
    pub max_tangential_accel: f32,
    pub min_speed: f32,
    pub max_speed: f32,
    /// Cosine of the largest roll away from world up.
    pub min_up_dot: f32,
    /// The camera must be able to see at least this far ahead...
    pub min_sightline: f32,
    /// ...but only sustained. Brushing past a doorway reveal briefly puts a
    /// wall in front of the lens and that is normal; holding a blank wall for
    /// this long is a bad shot.
    pub min_sightline_hold: f32,
    /// A shot may point at open sky for at most this long. Pointing at nothing
    /// passes every geometric check — the view is gloriously unobstructed —
    /// and is exactly as bad a shot as pointing at a wall.
    pub max_empty_hold: f32,
    /// How near a portal a room-to-room crossing must happen.
    pub portal_tolerance: f32,
    /// Sampling step for the position checks, seconds.
    pub dt: f32,
    /// Arc-length window the motion derivatives are measured over, metres.
    pub accel_window: f32,
}

impl Default for QaLimits {
    fn default() -> Self {
        QaLimits {
            min_clearance: 0.10,
            near_plane: 0.05,
            max_gaze_rate: 1.60,
            max_lateral_accel: 4.0,
            max_tangential_accel: 6.0,
            min_speed: 0.02,
            max_speed: 14.0,
            min_up_dot: 0.90,
            min_sightline: 0.45,
            min_sightline_hold: 0.6,
            max_empty_hold: 2.5,
            portal_tolerance: 1.6,
            dt: 1.0 / 60.0,
            accel_window: crate::path::ACCEL_WINDOW,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QaKind {
    InsideGeometry,
    NearPlane,
    GazeRate,
    LateralAccel,
    TangentialAccel,
    DeadStop,
    TooFast,
    Rolled,
    StaringAtWall,
    LookingAtNothing,
    WallCrossing,
    Empty,
}

impl QaKind {
    pub fn label(self) -> &'static str {
        match self {
            QaKind::InsideGeometry => "inside geometry",
            QaKind::NearPlane => "near plane clips",
            QaKind::GazeRate => "gaze too fast",
            QaKind::LateralAccel => "corner too tight",
            QaKind::TangentialAccel => "speed change too abrupt",
            QaKind::DeadStop => "dead stop",
            QaKind::TooFast => "too fast",
            QaKind::Rolled => "camera rolled",
            QaKind::StaringAtWall => "staring at a wall",
            QaKind::LookingAtNothing => "framing empty sky",
            QaKind::WallCrossing => "changed room without a door",
            QaKind::Empty => "empty track",
        }
    }
}

#[derive(Clone, Debug)]
pub struct QaFailure {
    pub t: f32,
    pub kind: QaKind,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct QaReport {
    pub track: String,
    pub kind_label: String,
    pub frames: usize,
    pub duration: f32,
    pub length: f32,
    pub min_clearance: f32,
    pub max_gaze_rate: f32,
    pub max_lateral_accel: f32,
    pub max_tangential_accel: f32,
    pub min_speed: f32,
    pub max_speed: f32,
    pub min_sightline: f32,
    pub rooms_visited: Vec<usize>,
    pub doors_passed: usize,
    pub failures: Vec<QaFailure>,
}

impl QaReport {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }

    /// One line, for a build log.
    pub fn summary(&self) -> String {
        format!(
            "{:<24} {:>6.1}s {:>6.1}m  clear {:>5.2}m  gaze {:>4.2}rad/s  lat {:>4.2}  spd {:>4.2}-{:>4.2}  rooms {:>2}  doors {:>2}  {}",
            self.track,
            self.duration,
            self.length,
            self.min_clearance,
            self.max_gaze_rate,
            self.max_lateral_accel,
            self.min_speed,
            self.max_speed,
            self.rooms_visited.len(),
            self.doors_passed,
            if self.passed() {
                "PASS".to_string()
            } else {
                format!("FAIL ({})", self.failures.len())
            }
        )
    }
}

/// Which storey's plan a world Z belongs to — the one whose eye height it is
/// nearest, within half a storey.
fn storey_for_z(site: &SiteAnalysis, z: f32) -> Option<usize> {
    let mut best = None;
    let mut bd = f32::INFINITY;
    for (i, st) in site.storeys.iter().enumerate() {
        let d = (z - st.eye_z).abs();
        // Tight on purpose. A loose band attributes a camera halfway up a
        // staircase to one of the floors it is between, and the climb then
        // reads as a room change with no door in it.
        if d < bd && d < 0.8 {
            bd = d;
            best = Some(i);
        }
    }
    best
}

pub fn check(site: &SiteAnalysis, track: &CameraTrack, limits: &QaLimits) -> QaReport {
    let mut r = QaReport {
        track: track.name.clone(),
        kind_label: track.kind_label.clone(),
        min_clearance: f32::INFINITY,
        min_speed: f32::INFINITY,
        min_sightline: f32::INFINITY,
        ..Default::default()
    };
    if track.keys.len() < 2 {
        r.failures.push(QaFailure {
            t: 0.0,
            kind: QaKind::Empty,
            detail: "track has fewer than two keys".into(),
        });
        return r;
    }
    r.duration = track.duration();
    r.length = track.path_length();

    let fly = site.clearance(ClearMode::Fly);
    let dt = limits.dt.max(1e-3);
    let n = ((r.duration / dt).ceil() as usize).max(2);
    r.frames = n + 1;

    let mut samples: Vec<(f32, Vec3f, Vec3f, Vec3f)> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = (i as f32 * dt).min(r.duration);
        // The last step is usually a partial one, and clamping it to the
        // duration would repeat the previous sample. Two samples at the same
        // instant read as a full stop followed by an infinite acceleration —
        // an artefact of the sampler, not a fault in the shot.
        if let Some((pt, _, _, _)) = samples.last() {
            if t - *pt < dt * 0.5 {
                continue;
            }
        }
        if let Some(k) = track.sample(t) {
            samples.push((t, k.pos, k.dir(), k.up));
        }
    }
    r.frames = samples.len();

    let mut last_room: Option<usize> = None;
    let mut blind_since: Option<f32> = None;
    let mut empty_since: Option<f32> = None;
    let fail = |r: &mut QaReport, t: f32, kind: QaKind, detail: String| {
        // One failure per kind is enough to act on; keep a few for context.
        if r.failures.iter().filter(|f| f.kind == kind).count() < 4 {
            r.failures.push(QaFailure { t, kind, detail });
        }
    };

    for i in 0..samples.len() {
        let (t, p, d, up) = samples[i];

        // -- correctness --------------------------------------------------
        let c = fly.at(p);
        r.min_clearance = r.min_clearance.min(c);
        if c < limits.min_clearance {
            fail(
                &mut r,
                t,
                QaKind::InsideGeometry,
                format!("clearance {c:.3} m < {:.3} m at {p:?}", limits.min_clearance),
            );
        }
        if c < limits.near_plane * 1.5 {
            fail(
                &mut r,
                t,
                QaKind::NearPlane,
                format!("clearance {c:.3} m within near plane {:.3} m", limits.near_plane),
            );
        }

        // -- comfort ------------------------------------------------------
        if up.normalize().dot(vec3(0.0, 0.0, 1.0)) < limits.min_up_dot {
            fail(&mut r, t, QaKind::Rolled, format!("up {up:?}"));
        }
        // -- is there anything to look at? --------------------------------
        let (see, _) = site.grid.sight_run(p, d, 150.0);
        r.min_sightline = r.min_sightline.min(see);
        if see < limits.min_sightline {
            blind_since = blind_since.or(Some(t));
            if let Some(t0) = blind_since {
                if t - t0 > limits.min_sightline_hold {
                    fail(
                        &mut r,
                        t,
                        QaKind::StaringAtWall,
                        format!("{see:.2} m of view for {:.1} s", t - t0),
                    );
                }
            }
        } else {
            blind_since = None;
        }

        // Is there anything in frame at all? `sight_run` stops at the first
        // thing you cannot see through, so a ray that runs its whole length is
        // a camera pointed at the sky.
        if see >= 149.0 {
            empty_since = empty_since.or(Some(t));
            if let Some(t0) = empty_since {
                if t - t0 > limits.max_empty_hold {
                    fail(
                        &mut r,
                        t,
                        QaKind::LookingAtNothing,
                        format!("nothing in frame for {:.1} s", t - t0),
                    );
                }
            }
        } else {
            empty_since = None;
        }

        // -- truthfulness: rooms change only through doors -----------------
        // Only a camera that is actually indoors can be *in* a room. A drone
        // orbiting overhead passes over the plan cells of every room in the
        // building, and without this it reports crossing walls that are twenty
        // metres beneath it.
        let indoors = site
            .grid
            .cell_of(p)
            .map_or(false, |(x, y, z)| !site.grid.exterior_at(x, y, z));
        if let Some(si) = storey_for_z(site, p.z).filter(|_| indoors) {
            if let Some(room) = site.room_at(p, si) {
                if !r.rooms_visited.contains(&room) {
                    r.rooms_visited.push(room);
                }
                if let Some(prev) = last_room {
                    if prev != room {
                        let via = site.portals.iter().find(|pt| {
                            ((pt.a == prev && pt.b == room) || (pt.a == room && pt.b == prev))
                                && (pt.center - p).length() < limits.portal_tolerance
                        });
                        // A stair is a legitimate way between storeys. Match
                        // at storey level, not room level: which room a flight
                        // is entered from depends on where the route joined it.
                        let (sp, sr) = (site.rooms[prev].storey, site.rooms[room].storey);
                        let stair = sp != sr
                            && site.stairs.iter().any(|s| {
                                let (a, b) = (
                                    site.rooms[s.lower_room].storey,
                                    site.rooms[s.upper_room].storey,
                                );
                                (a == sp && b == sr) || (a == sr && b == sp)
                            });
                        if via.is_some() {
                            r.doors_passed += 1;
                        } else if !stair {
                            fail(
                                &mut r,
                                t,
                                QaKind::WallCrossing,
                                format!(
                                    "{} → {} with no opening within {:.1} m",
                                    site.rooms[prev].name,
                                    site.rooms[room].name,
                                    limits.portal_tolerance
                                ),
                            );
                        }
                    }
                }
                last_room = Some(room);
            }
        }
    }

    // Motion is measured from the keys themselves, at their own times.
    // A `CameraTrack` is *defined* as keys with linear interpolation between
    // them, so sampling it finer than its key rate and taking second
    // differences measures the sampling — a piecewise-linear path has all its
    // acceleration concentrated at the keys — rather than the movement a
    // viewer would see.
    let keys = &track.keys;
    let nk = keys.len();
    let mut karc = vec![0f32; nk];
    for i in 1..nk {
        karc[i] = karc[i - 1] + (keys[i].pos - keys[i - 1].pos).length();
    }
    // Measure over a fixed *arc-length* window rather than between adjacent
    // keys. A `CameraTrack` is a polyline, so its direction changes in steps at
    // the keys; differencing adjacent keys concentrates each step into one
    // sample and reports an acceleration that scales with how finely the track
    // happens to be sampled rather than with what a viewer sees. Curvature over
    // a fixed distance is sampling-independent, and is the same quantity the
    // generator's speed limiter controls.
    // Gaze rate belongs with the other motion measurements, between real keys
    // at their real times, for the same reason: between keys the direction is
    // interpolated, so sampling finer only measures the interpolation.
    for i in 1..nk {
        let dt_k = (keys[i].t - keys[i - 1].t).max(1e-5);
        let rate = angle_between(keys[i - 1].dir(), keys[i].dir()) / dt_k;
        r.max_gaze_rate = r.max_gaze_rate.max(rate);
        if rate > limits.max_gaze_rate {
            fail(
                &mut r,
                keys[i].t,
                QaKind::GazeRate,
                format!("{rate:.2} rad/s > {:.2}", limits.max_gaze_rate),
            );
        }
    }

    let w = limits.accel_window.max(1e-3);
    for i in 0..nk {
        let Some(a) = (0..i).rev().find(|j| karc[i] - karc[*j] >= w) else {
            continue;
        };
        let Some(b) = ((i + 1)..nk).find(|j| karc[*j] - karc[i] >= w) else {
            continue;
        };
        let dt_ab = (keys[b].t - keys[a].t).max(1e-5);
        let ds_ab = karc[b] - karc[a];
        let speed = ds_ab / dt_ab;
        r.min_speed = r.min_speed.min(speed);
        r.max_speed = r.max_speed.max(speed);
        if speed < limits.min_speed {
            fail(&mut r, keys[i].t, QaKind::DeadStop, format!("{speed:.3} m/s"));
        }
        if speed > limits.max_speed {
            fail(&mut r, keys[i].t, QaKind::TooFast, format!("{speed:.2} m/s"));
        }

        let k = curvature(keys[a].pos, keys[i].pos, keys[b].pos);
        let lat = speed * speed * k;
        r.max_lateral_accel = r.max_lateral_accel.max(lat);
        if lat > limits.max_lateral_accel {
            fail(
                &mut r,
                keys[i].t,
                QaKind::LateralAccel,
                format!("{lat:.2} m/s² > {:.2}", limits.max_lateral_accel),
            );
        }

        let va = (karc[i] - karc[a]) / (keys[i].t - keys[a].t).max(1e-5);
        let vb = (karc[b] - karc[i]) / (keys[b].t - keys[i].t).max(1e-5);
        let tan = (vb - va) / (dt_ab * 0.5);
        r.max_tangential_accel = r.max_tangential_accel.max(tan.abs());
        if tan.abs() > limits.max_tangential_accel {
            fail(
                &mut r,
                keys[i].t,
                QaKind::TangentialAccel,
                format!("{:.2} m/s² > {:.2}", tan.abs(), limits.max_tangential_accel),
            );
        }
    }

    if !r.min_clearance.is_finite() {
        r.min_clearance = 0.0;
    }
    if !r.min_speed.is_finite() {
        r.min_speed = 0.0;
    }
    if !r.min_sightline.is_finite() {
        r.min_sightline = 0.0;
    }
    r
}

/// Check a whole set of tracks.
pub fn check_all(
    site: &SiteAnalysis,
    tracks: &[CameraTrack],
    limits: &QaLimits,
) -> Vec<QaReport> {
    tracks.iter().map(|t| check(site, t, limits)).collect()
}

/// Times worth showing a human: the track's own notes, plus an even spread so
/// a contact sheet covers the whole shot even when a generator emits no notes.
pub fn keyframe_times(track: &CameraTrack, count: usize) -> Vec<f32> {
    let d = track.duration();
    let mut ts: Vec<f32> = track.notes.iter().map(|n| n.t).collect();
    let want = count.max(2);
    if ts.len() < want {
        let extra = want - ts.len();
        for i in 0..extra {
            ts.push(d * (i as f32 + 0.5) / extra as f32);
        }
    }
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ts.dedup_by(|a, b| (*a - *b).abs() < d * 0.02);
    ts.truncate(want.max(1));
    ts
}

/// Reciprocal of the radius of the circle through three points.
fn curvature(a: V3, b: V3, c: V3) -> f32 {
    let ab = b - a;
    let cb = b - c;
    let cross = V3::cross(ab, cb).length();
    let denom = ab.length() * cb.length() * (a - c).length();
    if denom < 1e-8 {
        0.0
    } else {
        2.0 * cross / denom
    }
}
