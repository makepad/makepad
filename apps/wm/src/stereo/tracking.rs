//! Head tracking for the VR view, independent of any device.
//!
//! A `PoseSource` is one tracker: a device protocol backend with its own
//! worker (the Beyond and XREAL IMUs in `crate::headset_imu`; a
//! runtime-backed Lighthouse tracker can be added later). Each says what it
//! can measure (`Tracking::Orientation` for 3DOF, `Tracking::Full` for
//! 6DOF) and hands out its newest pose without waiting. The
//! `HeadTracker` asks its sources in order and uses the best pose
//! available this frame: a full (6DOF) pose wins, an orientation-only one
//! is the fallback, with the position held at the fixed seated origin.
//! Nothing here derives a position from accelerations.
//!
//! The renderer sees only `TrackedPose`; it never knows which device or
//! protocol produced it.

use std::sync::Arc;

use makepad_widgets::*;

use super::calibration::DisplayCalibration;

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

/// The sources in priority order and the pose they last gave.
#[derive(Default)]
pub struct HeadTracker {
    selected_display: Option<([u8; 3], u16)>,
    sources: Vec<Box<dyn PoseSource>>,
    /// The source the last pose came from, for logging a change.
    current: Option<usize>,
    /// The last tracked pose: held (as `Tracking::None`) while every source
    /// is stale, so a tracker dropping out freezes the view instead of
    /// snapping it to straight ahead. Recenter clears it.
    held: TrackedPose,
    /// The sources' display generations last seen, and the calibration
    /// taken then (the first source that has one).
    display_seen: Vec<u32>,
    display: Option<Arc<DisplayCalibration>>,
}

impl HeadTracker {
    /// The trackers this build and machine offer. Today: the headset IMUs
    /// (3DOF; `crate::headset_imu` picks the device). A 6DOF source
    /// (Lighthouse through an OpenXR runtime) needs a desktop OpenXR path in
    /// the platform first; see the lane's proposal.
    pub fn start(cx: &mut Cx, display: Option<([u8; 3], u16)>) -> Self {
        let imu = crate::headset_imu::HeadsetImu::start(
            &cx.thread_spawner(),
            imu_source(display),
        );
        Self {
            selected_display: display,
            sources: vec![Box::new(ImuSource(imu))],
            current: None,
            held: TrackedPose::default(),
            display_seen: Vec::new(),
            display: None,
        }
    }

    pub fn select_display(&mut self, display: Option<([u8; 3], u16)>) {
        // A mode switch temporarily removes its connector. Keep the chosen
        // device while VR remains active: dropping it here would restore
        // mono during our own SBS hotplug and reset the tracked heading.
        // The worker detects a USB disconnect; a different display retargets
        // it, and closing VR drops it and restores the original mode.
        if display.is_none() && self.selected_display.is_some() { return; }
        if self.selected_display != display {
            self.selected_display = display;
            self.held = TrackedPose::default();
            for source in &self.sources { source.select_display(display); }
        }
    }

    /// The best pose any source has now: `Full` over `Orientation`; the
    /// first source wins a tie. When none has one, the last pose held with
    /// `Tracking::None`.
    pub fn pose(&mut self, now: f64, display_time: f64) -> TrackedPose {
        let mut best: Option<(usize, TrackedPose)> = None;
        for (i, source) in self.sources.iter().enumerate() {
            if let Some(mut pose) = source.pose_at(now, display_time) {
                if pose.tracking != Tracking::Full {
                    pose.position = [0.0; 3];
                }
                if best.map_or(true, |(_, b)| pose.tracking > b.tracking) {
                    best = Some((i, pose));
                }
            }
        }
        let index = best.map(|(i, _)| i);
        if index != self.current {
            self.current = index;
            match index {
                Some(i) => log!("wm: vr tracking from {}", self.sources[i].label()),
                None => log!("wm: vr tracking lost; the view holds its last pose"),
            }
        }
        match best.map(|(_, p)| p).filter(|p| p.tracking != Tracking::None) {
            Some(pose) => {
                self.held = pose;
                pose
            }
            None => TrackedPose { tracking: Tracking::None, ..self.held },
        }
    }

    /// The headset display's optics from the first source that reports
    /// them, and whether that changed since the last call. Per frame: one
    /// atomic load per source; a source's lock only on its change.
    pub fn display_calibration(&mut self) -> (bool, Option<Arc<DisplayCalibration>>) {
        let changed = self.display_seen.len() != self.sources.len()
            || self.sources.iter().zip(&self.display_seen).any(|(s, seen)| s.display_generation() != *seen);
        if changed {
            self.display_seen = self.sources.iter().map(|s| s.display_generation()).collect();
            self.display = self.sources.iter().find_map(|s| s.display_calibration());
        }
        (changed, self.display.clone())
    }

    pub fn recenter(&mut self) {
        self.held = TrackedPose::default();
        for source in &self.sources {
            source.recenter();
        }
    }

    pub fn vsync_to_photons(&self) -> Option<f64> {
        self.display.as_ref()?.vsync_to_photons
    }

    /// One line per source.
    pub fn status(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|s| format!("{} ({:?}): {}", s.label(), s.capability(), s.status()))
            .collect()
    }
}

/// An IMU pose older than this (seconds) is stale: the stream stopped.
const IMU_STALE: f64 = 0.1;

/// The IMU backends in `crate::headset_imu` share orientation-only fusion.
struct ImuSource(crate::headset_imu::HeadsetImu);

fn imu_source(display: Option<([u8; 3], u16)>) -> crate::headset_imu::HeadsetImuSource {
    use crate::headset_imu::HeadsetImuSource;
    display.map_or(HeadsetImuSource::Auto, |(vendor, product)| HeadsetImuSource::Display(vendor, product))
}

impl PoseSource for ImuSource {
    fn select_display(&self, display: Option<([u8; 3], u16)>) {
        self.0.select_source(imu_source(display));
    }
    fn label(&self) -> &'static str {
        "headset IMU"
    }

    fn capability(&self) -> Tracking {
        Tracking::Orientation
    }

    fn pose(&self, now: f64) -> Option<TrackedPose> {
        self.pose_at(now, now)
    }

    fn pose_at(&self, now: f64, display_time: f64) -> Option<TrackedPose> {
        // Fresh samples only (the IMU streams at about 1 kHz): judged by the
        // sample time, so the per-frame path takes no lock and allocates
        // nothing (`status()` does both).
        let latest = self.0.latest()?;
        let q = latest.predicted(display_time);
        makepad_widgets::makepad_platform::trace!("wm.vr", "pose now={:.6} sample={:.6} age_ms={:.3} samples={} rate={:.3} predict_ms={:.3} q={:?}",
            now, latest.time, (now - latest.time) * 1000.0, latest.samples,
            latest.angular_velocity.length(), (display_time - latest.time) * 1000.0, q);
        if now - latest.time > IMU_STALE {
            return None;
        }
        Some(TrackedPose {
            orientation: [q.x, q.y, q.z, q.w],
            position: [0.0; 3],
            tracking: Tracking::Orientation,
        })
    }

    fn recenter(&self) {
        self.0.recenter();
    }

    fn display_generation(&self) -> u32 {
        self.0.display_generation()
    }

    fn display_calibration(&self) -> Option<Arc<DisplayCalibration>> {
        self.0.display_calibration()
    }

    fn status(&self) -> String {
        let status = self.0.status();
        format!("{:?} {}", status.state, status.detail)
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
