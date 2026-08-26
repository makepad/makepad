//! Lane C. Orbit-mode camera math, pure functions over [`Camera`].
//!
//! Turntable is the default: the boom is re-derived from `eye - target` on
//! every step and the world up is re-asserted, so a mixed sequence of drags
//! can never accumulate roll (the classic "my horizon is tilted" bug). The
//! trackball style is a real free rotation about the two screen axes and does
//! let the horizon tip — that is the point of it.
//!
//! Dolly is *to the cursor*: the camera is uniformly scaled about a point on
//! the ray under the pointer, which leaves every point of that ray projecting
//! to exactly the same pixel. See `dolly_keeps_cursor_fixed` in the tests.

use crate::api::*;
use makepad_widgets::*;

/// Just short of the pole: a camera exactly on the axis has no defined yaw.
pub const PITCH_LIMIT: f32 = 1.5533; // 89°
pub const MIN_DISTANCE: f32 = 0.02;
pub const MAX_DISTANCE: f32 = 40_000.0;
pub const MIN_ORTHO_HEIGHT: f32 = 0.02;
pub const MAX_ORTHO_HEIGHT: f32 = 80_000.0;
/// Radians of orbit per layout point of pointer travel (Fab's rate).
pub const ORBIT_SENS: f32 = 0.0075;

pub const WORLD_UP: Vec3f = Vec3f {
    x: 0.0,
    y: 0.0,
    z: 1.0,
};

/// Rodrigues rotation of `v` about `axis` by `angle` radians.
pub fn rotate_about(v: Vec3f, axis: Vec3f, angle: f32) -> Vec3f {
    let a = axis.normalize();
    if !a.is_finite() {
        return v;
    }
    let s = angle.sin();
    let c = angle.cos();
    v * c + Vec3f::cross(a, v) * s + a * (a.dot(v) * (1.0 - c))
}

/// Some unit vector perpendicular to `v`.
fn any_perpendicular(v: Vec3f) -> Vec3f {
    let a = if v.x.abs() < 0.9 {
        vec3(1.0, 0.0, 0.0)
    } else {
        vec3(0.0, 1.0, 0.0)
    };
    Vec3f::cross(v, a).normalize()
}

/// Shortest-arc interpolation between two unit vectors.
pub fn slerp(a: Vec3f, b: Vec3f, f: f32) -> Vec3f {
    let a = a.normalize();
    let b = b.normalize();
    if !a.is_finite() || !b.is_finite() {
        return b;
    }
    let d = a.dot(b).clamp(-1.0, 1.0);
    if d > 0.9995 {
        return Vec3f::from_lerp(a, b, f).normalize();
    }
    if d < -0.9995 {
        // Antipodal: no shortest arc exists, take a fixed half-turn plane.
        return rotate_about(a, any_perpendicular(a), std::f32::consts::PI * f);
    }
    let theta = d.acos();
    let st = theta.sin();
    a * (((1.0 - f) * theta).sin() / st) + b * ((f * theta).sin() / st)
}

/// The turntable angles the camera is currently at, `(yaw, pitch)` radians.
/// At the poles the yaw is read back out of the up vector, so entering the
/// Top / Bottom preset and then orbiting continues from the right azimuth
/// instead of snapping to yaw 0.
pub fn turntable_angles(cam: &Camera) -> (f32, f32) {
    let offset = cam.eye - cam.target;
    let dist = offset.length().max(1e-5);
    let sin_pitch = (offset.z / dist).clamp(-1.0, 1.0);
    let pitch = sin_pitch.asin();
    let horiz = (offset.x * offset.x + offset.y * offset.y).sqrt();
    let yaw = if horiz > dist * 1e-3 {
        offset.y.atan2(offset.x)
    } else {
        let u = cam.true_up();
        let s = if sin_pitch >= 0.0 { -1.0 } else { 1.0 };
        if u.is_finite() && (u.x.abs() + u.y.abs()) > 1e-5 {
            (s * u.y).atan2(s * u.x)
        } else {
            0.0
        }
    };
    (yaw, pitch)
}

/// Place the eye from turntable angles, world up re-asserted.
pub fn set_turntable(cam: &mut Camera, yaw: f32, pitch: f32, dist: f32) {
    let pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    let dist = dist.clamp(MIN_DISTANCE, MAX_DISTANCE);
    let cp = pitch.cos();
    cam.eye = cam.target + vec3(cp * yaw.cos(), cp * yaw.sin(), pitch.sin()) * dist;
    cam.up = WORLD_UP;
}

/// Turntable orbit by a screen delta in layout points. Up-locked: the world
/// up is re-asserted every step, so roll can never accumulate.
pub fn orbit_turntable(cam: &mut Camera, dx: f32, dy: f32) {
    let (yaw, pitch) = turntable_angles(cam);
    let dist = cam.distance();
    set_turntable(cam, yaw - dx * ORBIT_SENS, pitch + dy * ORBIT_SENS, dist);
}

/// Free trackball orbit about the two screen axes (roll is allowed).
pub fn orbit_trackball(cam: &mut Camera, dx: f32, dy: f32) {
    let right = cam.right();
    let up = cam.true_up();
    if !right.is_finite() || !up.is_finite() {
        orbit_turntable(cam, dx, dy);
        return;
    }
    let mut offset = cam.eye - cam.target;
    let mut new_up = cam.up;
    let yaw = -dx * ORBIT_SENS;
    let pitch = -dy * ORBIT_SENS;
    offset = rotate_about(offset, up, yaw);
    offset = rotate_about(offset, right, pitch);
    new_up = rotate_about(new_up, up, yaw);
    new_up = rotate_about(new_up, right, pitch);
    cam.eye = cam.target + offset;
    let n = new_up.normalize();
    if n.is_finite() {
        cam.up = n;
    }
}

/// Screen-plane pan: drag the world with the pointer, 1:1 at the pivot depth.
pub fn pan(cam: &mut Camera, dx: f32, dy: f32, rect_h: f32) {
    let world_per_point = if cam.ortho {
        cam.ortho_height / rect_h.max(1.0)
    } else {
        2.0 * cam.distance() * (cam.fov_y_deg.to_radians() * 0.5).tan() / rect_h.max(1.0)
    };
    let shift = cam.right() * (-dx * world_per_point) + cam.true_up() * (dy * world_per_point);
    if !shift.is_finite() {
        return;
    }
    cam.eye += shift;
    cam.target += shift;
}

/// The point where a ray crosses the plane through the pivot facing the
/// camera — the fallback dolly anchor when nothing was picked.
pub fn anchor_on_pivot_plane(cam: &Camera, ray: &Ray) -> Vec3f {
    let n = cam.forward();
    let denom = n.dot(ray.dir);
    if denom.abs() < 1e-6 {
        return cam.target;
    }
    let t = (cam.target - ray.origin).dot(n) / denom;
    if !t.is_finite() || t <= 0.0 {
        return cam.target;
    }
    ray.at(t)
}

/// Dolly by `factor` (< 1 moves in) about `anchor`.
///
/// Perspective: eye *and* pivot are scaled about the anchor. The orientation
/// never changes and the eye stays on the line through the anchor, so every
/// world point on the cursor ray keeps its exact pixel. Orthographic: the
/// height scales and the camera slides sideways by the matching amount.
pub fn dolly(cam: &mut Camera, factor: f32, anchor: Option<Vec3f>) {
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    if cam.ortho {
        let h = cam.ortho_height.max(1e-4);
        let f = factor.clamp(MIN_ORTHO_HEIGHT / h, MAX_ORTHO_HEIGHT / h);
        cam.ortho_height = h * f;
        if let Some(a) = anchor {
            let fwd = cam.forward();
            let v = cam.eye - a;
            let lateral = v - fwd * v.dot(fwd);
            let shift = lateral * (f - 1.0);
            if shift.is_finite() {
                cam.eye += shift;
                cam.target += shift;
            }
        }
    } else {
        let dist = cam.distance().max(1e-5);
        let f = factor.clamp(MIN_DISTANCE / dist, MAX_DISTANCE / dist);
        let a = anchor.unwrap_or(cam.target);
        let eye = a + (cam.eye - a) * f;
        let target = a + (cam.target - a) * f;
        if eye.is_finite() && target.is_finite() {
            cam.eye = eye;
            cam.target = target;
        }
    }
}

/// Move the pivot to `point` without turning: the whole camera translates, so
/// the framing slides but the direction and distance survive. This is what a
/// double-click on a surface does.
/// Orbit around what the pointer is over: the pivot moves to `point` and the
/// EYE STAYS WHERE IT IS, so the picture does not shift — only the centre the
/// turntable swings about. Orbiting a house from across the site otherwise
/// swings around the model origin, which throws the thing you are looking at
/// off screen; anchoring on the surface under the cursor is what makes an
/// orbit feel like turning an object in the hand.
pub fn set_pivot(cam: &mut Camera, point: Vec3f) {
    if !point.is_finite() {
        return;
    }
    let dist = (cam.eye - point).length();
    if !dist.is_finite() || dist < MIN_DISTANCE || dist > MAX_DISTANCE {
        return;
    }
    cam.target = point;
    cam.focus_distance = dist;
}

pub fn recenter(cam: &mut Camera, point: Vec3f) {
    let shift = point - cam.target;
    if !shift.is_finite() {
        return;
    }
    cam.eye += shift;
    cam.target = point;
}

/// Switch projection while keeping the apparent size of whatever is at the
/// pivot. Instant on purpose — a projection cannot be blended.
pub fn set_ortho(cam: &mut Camera, ortho: bool) {
    if cam.ortho == ortho {
        return;
    }
    let half_fov = (cam.fov_y_deg.to_radians() * 0.5).max(1e-4);
    if ortho {
        cam.ortho_height = (2.0 * cam.distance() * half_fov.tan())
            .clamp(MIN_ORTHO_HEIGHT, MAX_ORTHO_HEIGHT);
    } else {
        let d = (cam.ortho_height * 0.5 / half_fov.tan()).clamp(MIN_DISTANCE, MAX_DISTANCE);
        let dir = cam.forward();
        if dir.is_finite() {
            cam.eye = cam.target - dir * d;
        }
    }
    cam.ortho = ortho;
}

/// The camera a preset view wants, keeping the current pivot and distance.
/// Axis-aligned presets go orthographic, matching Fab's auto-perspective.
pub fn preset_camera(cam: &Camera, preset: PresetView, auto_ortho: bool) -> Camera {
    let mut to = *cam;
    let (dir, up) = preset.look_dir_and_up();
    let dist = cam.distance().clamp(MIN_DISTANCE, MAX_DISTANCE);
    to.eye = cam.target - dir * dist;
    to.up = up;
    if auto_ortho && preset != PresetView::Isometric {
        set_ortho(&mut to, true);
    }
    to
}

// ===========================================================================
// Eased transitions
// ===========================================================================

/// Smoothstep — an ease in and out with zero slope at both ends and no
/// overshoot. Fab's "smooth view", and the only easing this app uses.
pub fn ease_in_out(f: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

/// Blend two cameras: the pivot travels straight, the boom slerps and its
/// length interpolates geometrically, so a 180° flip sweeps instead of
/// passing through the model.
pub fn lerp_camera(a: &Camera, b: &Camera, f: f32) -> Camera {
    let mut c = *b;
    let da = a.distance().max(1e-4);
    let db = b.distance().max(1e-4);
    let dir_a = (a.eye - a.target) * (1.0 / da);
    let dir_b = (b.eye - b.target) * (1.0 / db);
    let dir = slerp(dir_a, dir_b, f);
    let dist = (da.ln() + (db.ln() - da.ln()) * f).exp();
    c.target = Vec3f::from_lerp(a.target, b.target, f);
    c.eye = c.target + dir * dist;
    let up = slerp(a.up, b.up, f);
    if up.is_finite() {
        c.up = up;
    }
    c.fov_y_deg = a.fov_y_deg + (b.fov_y_deg - a.fov_y_deg) * f;
    let ha = a.ortho_height.max(1e-4);
    let hb = b.ortho_height.max(1e-4);
    c.ortho_height = (ha.ln() + (hb.ln() - ha.ln()) * f).exp();
    c
}

/// One camera move in flight. Ends *exactly* on `to` so a preset view lands
/// on the nose rather than a smoothstep's worth of epsilon away.
#[derive(Clone, Copy, Debug)]
pub struct CameraAnim {
    pub from: Camera,
    pub to: Camera,
    pub t: f32,
    pub dur: f32,
    /// Re-applied after every step, because `mark_camera_changed` clears it.
    pub preset: Option<PresetView>,
}

impl CameraAnim {
    pub fn new(from: Camera, to: Camera, dur: f32, preset: Option<PresetView>) -> Self {
        let mut from = from;
        // A projection cannot be blended: enter the destination's projection
        // up front with a height that matches the current apparent size, so
        // the move is one continuous glide instead of a glide plus a pop.
        if to.ortho && !from.ortho {
            set_ortho(&mut from, true);
        } else if !to.ortho && from.ortho {
            set_ortho(&mut from, false);
        }
        CameraAnim {
            from,
            to,
            t: 0.0,
            dur: dur.max(0.0),
            preset,
        }
    }

    /// Advance by `dt`; returns the camera for this frame and whether it is
    /// the last one.
    pub fn step(&mut self, dt: f32) -> (Camera, bool) {
        self.t = (self.t + dt.max(0.0)).min(self.dur);
        if self.dur <= 1e-4 || self.t >= self.dur {
            return (self.to, true);
        }
        let f = ease_in_out(self.t / self.dur);
        (lerp_camera(&self.from, &self.to, f), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn house_cam() -> Camera {
        let mut c = Camera::default();
        c.target = vec3(5.0, 3.5, 2.5);
        c.eye = vec3(18.0, -20.0, 12.0);
        c.up = WORLD_UP;
        c
    }

    #[test]
    fn turntable_never_accumulates_roll() {
        let mut cam = house_cam();
        // Twenty mixed drags, deliberately including near-polar excursions.
        let drags = [
            (37.0, -12.0),
            (-90.0, 40.0),
            (5.0, 300.0),
            (250.0, -400.0),
            (-3.0, 3.0),
            (120.0, 120.0),
            (-260.0, -70.0),
            (11.0, -900.0),
            (0.0, 900.0),
            (-500.0, 0.0),
        ];
        for round in 0..2 {
            for (dx, dy) in drags {
                orbit_turntable(&mut cam, dx * (1.0 - round as f32 * 2.0), dy);
                assert!(cam.eye.is_finite(), "eye went non-finite");
                // Up stays world up and the horizon stays level: the camera
                // right axis has no vertical component at all.
                assert!(
                    cam.right().z.abs() < 1e-5,
                    "roll crept in: right = {:?}",
                    cam.right()
                );
            }
        }
        assert!((cam.up - WORLD_UP).length() < 1e-6);
    }

    #[test]
    fn turntable_survives_the_poles() {
        // Straight down with up = +Y. Orbiting must continue from that
        // azimuth, not snap the model a quarter turn.
        let mut cam = preset_camera(&house_cam(), PresetView::Top, false);
        let before = cam.forward();
        orbit_turntable(&mut cam, 0.0, -1.0);
        let after = cam.forward();
        assert!(after.is_finite());
        assert!(
            after.dot(before) > 0.999,
            "the top view jumped: {before:?} -> {after:?}"
        );
        assert!(cam.right().z.abs() < 1e-5);
    }

    #[test]
    fn dolly_keeps_the_cursor_point_fixed() {
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1600.0, 1000.0),
        };
        for ortho in [false, true] {
            for (cx, cy) in [
                (200.0, 150.0),
                (1400.0, 880.0),
                (800.0, 500.0),
                (60.0, 940.0),
            ] {
                for factor in [0.5f32, 0.8, 1.25, 2.0] {
                    let mut cam = house_cam();
                    if ortho {
                        set_ortho(&mut cam, true);
                    }
                    let proj = ViewProjector::new(cam, rect);
                    let ray = proj.ray(dvec2(cx, cy));
                    let anchor = anchor_on_pivot_plane(&cam, &ray);
                    let before = proj.project(anchor).expect("anchor is in front");
                    dolly(&mut cam, factor, Some(anchor));
                    let after = ViewProjector::new(cam, rect)
                        .project(anchor)
                        .expect("anchor stays in front");
                    let err = (after - before).length();
                    assert!(
                        err < 2.0,
                        "cursor point moved {err:.3} px (ortho={ortho}, f={factor})"
                    );
                }
            }
        }
    }

    #[test]
    fn ortho_toggle_keeps_apparent_size() {
        let mut cam = house_cam();
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(1600.0, 1000.0),
        };
        let probe = cam.target + cam.right() * 3.0;
        let before = ViewProjector::new(cam, rect).project(probe).unwrap();
        set_ortho(&mut cam, true);
        let mid = ViewProjector::new(cam, rect).project(probe).unwrap();
        assert!((mid - before).length() < 1.0, "{before:?} -> {mid:?}");
        set_ortho(&mut cam, false);
        let after = ViewProjector::new(cam, rect).project(probe).unwrap();
        assert!((after - before).length() < 1.0, "{before:?} -> {after:?}");
    }

    #[test]
    fn presets_land_exactly() {
        let cam = house_cam();
        for preset in [
            PresetView::Front,
            PresetView::Back,
            PresetView::Left,
            PresetView::Right,
            PresetView::Top,
            PresetView::Bottom,
            PresetView::Isometric,
        ] {
            let to = preset_camera(&cam, preset, true);
            let (dir, _) = preset.look_dir_and_up();
            assert!(
                to.forward().dot(dir) > 0.99999,
                "{preset:?}: {:?} vs {dir:?}",
                to.forward()
            );
            assert!((to.distance() - cam.distance()).abs() < 1e-3);
            // And the animation to it lands on that camera, not near it.
            let mut anim = CameraAnim::new(cam, to, 0.25, Some(preset));
            let mut steps = 0;
            loop {
                let (c, done) = anim.step(1.0 / 60.0);
                steps += 1;
                assert!(steps < 200, "animation never finished");
                if done {
                    assert_eq!(c.eye, to.eye);
                    assert_eq!(c.target, to.target);
                    assert_eq!(c.up, to.up);
                    assert_eq!(c.ortho, to.ortho);
                    break;
                }
                assert!(c.eye.is_finite(), "mid-flight eye went bad");
            }
        }
    }

    #[test]
    fn recenter_keeps_direction_and_distance() {
        let mut cam = house_cam();
        let dir = cam.forward();
        let dist = cam.distance();
        recenter(&mut cam, vec3(-2.0, 9.0, 1.0));
        assert!(cam.forward().dot(dir) > 0.99999);
        assert!((cam.distance() - dist).abs() < 1e-4);
        assert!((cam.target - vec3(-2.0, 9.0, 1.0)).length() < 1e-5);
    }
}
