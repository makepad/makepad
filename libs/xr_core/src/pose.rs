//! Head poses and the trackers that give them, independent of any device and of who owns the
//! headset display (the WM for a tethered headset, the HAL's OpenXR backend, a remote headset's
//! network link). Moved from the WM's `stereo::tracking`.
//!
//! A `PoseSource` is one tracker. Each says what it can measure (`Tracking::Orientation` for 3DOF,
//! `Tracking::Full` for 6DOF) and hands out its newest pose without waiting. The renderer sees only
//! `TrackedPose`; it never knows which device or protocol produced it.

use std::sync::Arc;

use crate::optics::DisplayCalibration;

/// How much of the pose is measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Tracking {
    /// Nothing: the view looks straight ahead.
    #[default]
    None,
    /// Orientation only (3DOF); the position is the fixed origin.
    Orientation,
    /// Orientation and position (6DOF).
    Full,
}

/// A head pose: x right, y up, z toward the viewer; the identity
/// orientation looks along -z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackedPose {
    /// Head-to-world rotation, unit quaternion `[x, y, z, w]`.
    pub orientation: [f32; 4],
    /// Head position, metres; `[0; 3]` unless `tracking` is `Full`.
    pub position: [f32; 3],
    pub tracking: Tracking,
}

impl Default for TrackedPose {
    fn default() -> Self {
        Self {
            orientation: [0.0, 0.0, 0.0, 1.0],
            position: [0.0; 3],
            tracking: Tracking::None,
        }
    }
}

/// One tracker. Every call is non-blocking: a source reads what its own
/// worker last published.
pub trait PoseSource {
    /// Short name for logs and the status line.
    fn label(&self) -> &'static str;
    /// The most this source can measure when it is working.
    fn capability(&self) -> Tracking;
    /// The pose predicted for `now` (`Cx::monotonic_now()`), or `None`
    /// while the source has nothing current.
    fn pose(&self, now: f64) -> Option<TrackedPose>;
    /// Sample now, but predict for the frame's display time. Sources that
    /// already have runtime-predicted poses can keep their own prediction.
    fn pose_at(&self, now: f64, _display_time: f64) -> Option<TrackedPose> {
        self.pose(now)
    }
    /// Make the current heading straight ahead.
    fn recenter(&self);
    /// What the source is doing, for the status line.
    fn status(&self) -> String;
    fn select_display(&self, _display: Option<([u8; 3], u16)>) {}
    /// Bumps when `display_calibration` may have changed (a device opened or
    /// went away); one atomic load, polled per frame.
    fn display_generation(&self) -> u32 {
        0
    }
    /// The optics of the headset display this tracker belongs to, when its
    /// device reports them. May take a short lock: call on a generation
    /// change only.
    fn display_calibration(&self) -> Option<Arc<DisplayCalibration>> {
        None
    }
}

// ----------------------------------------------------------------------
// Rotation helpers (unit quaternions [x, y, z, w], column-vector matrices)
// ----------------------------------------------------------------------

pub type Quat4 = [f32; 4];

pub fn quat_mul(a: Quat4, b: Quat4) -> Quat4 {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

pub fn quat_normalize(q: Quat4) -> Quat4 {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n < 1e-6 || !n.is_finite() {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
}

/// A rotation of `angle` radians about the unit `axis`.
pub fn quat_axis_angle(axis: [f32; 3], angle: f32) -> Quat4 {
    let (s, c) = (angle * 0.5).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// The rotation matrix's columns: the head's right, up and back axes in
/// world space.
pub fn quat_columns(q: Quat4) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)],
        [2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)],
        [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}
