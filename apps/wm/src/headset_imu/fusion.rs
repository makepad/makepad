//! Orientation from an IMU, independent of the device: gyro integration
//! held to gravity, with the gyro bias learnt while the head is still.
//!
//! A backend converts its reports into [`ImuSample`]s in the head frame
//! (x right, y up, z toward the viewer, so the head looks along -z) in SI
//! units, with the step from its own clock. Nothing here measures heading:
//! pitch and roll follow gravity, yaw is integrated gyro and drifts (slowly
//! once the bias estimate has settled, but without bound). [`OrientationFilter::recenter`]
//! makes the current heading forward again.
use makepad_widgets::{Quat, Vec3f};

/// Gravity correction gain (rad/s per unit of direction error): fast while
/// the first estimate settles, then slow so head accelerations barely tilt
/// the horizon.
const GRAVITY_GAIN_START: f32 = 5.0;
const GRAVITY_GAIN: f32 = 0.3;
const START_SECONDS: f32 = 1.0;
/// At rest (rate under `STILL_RATE`, |a| within `STILL_ACCEL` of gravity
/// for `STILL_SECONDS`) the gyro reading is bias and is low-passed into
/// the bias estimate: with time constant `BIAS_TAU_START` for the first
/// `BIAS_WARMUP` seconds of rest, then `BIAS_TAU`. Until the bias is known
/// yaw drifts by it and the horizon sits off by bias / gain (a Beyond 2's
/// 0.5 °/s at rest is 1.8° at the slow gain).
const STILL_RATE: f32 = 0.05;
const STILL_ACCEL: f32 = 0.03;
const STILL_SECONDS: f32 = 0.5;
const BIAS_TAU_START: f32 = 0.5;
const BIAS_WARMUP: f32 = 3.0;
const BIAS_TAU: f32 = 4.0;

const WORLD_UP: Vec3f = Vec3f { x: 0.0, y: 1.0, z: 0.0 };

/// One IMU reading in the head frame.
#[derive(Clone, Copy, Debug)]
pub struct ImuSample {
    /// Angular velocity, rad/s.
    pub gyro: Vec3f,
    /// Specific force, m/s² (points up at rest).
    pub accel: Vec3f,
    /// Seconds since the previous sample on the device's clock; 0 after a
    /// gap or for the first sample (nothing is integrated across it).
    pub dt: f32,
}

#[derive(Default)]
pub struct OrientationFilter {
    /// Head to gravity-aligned world.
    q: Quat,
    started: bool,
    /// Seconds of samples fused.
    elapsed: f32,
    bias: Vec3f,
    /// Mean |a| at rest.
    gravity: f32,
    still: f32,
    /// Seconds of rest the bias has been learnt over.
    bias_seconds: f32,
    /// The heading taken out by the last recenter.
    yaw_ref: Quat,
    /// The bias-corrected rate of the last sample.
    rate: Vec3f,
}

impl OrientationFilter {
    /// True once the first gravity fix has set pitch and roll.
    pub fn started(&self) -> bool {
        self.started
    }

    /// Head-to-world rotation with the recentered heading taken out. The
    /// first fix turns gravity's direction straight up, which leaves the
    /// heading about where the head points.
    pub fn orientation(&self) -> Quat {
        // yaw_ref⁻¹ ⊗ q (`Quat::multiply(a, b)` is b ⊗ a).
        Quat::multiply(&self.q, &self.yaw_ref.invert()).normalized()
    }

    /// The last sample's angular velocity, head frame, bias removed.
    pub fn rate(&self) -> Vec3f {
        self.rate
    }

    /// Make the current heading forward; pitch and roll stay on gravity.
    pub fn recenter(&mut self) {
        self.yaw_ref = heading(self.q);
    }

    pub fn update(&mut self, sample: &ImuSample) {
        let (gyro, acc, dt) = (sample.gyro, sample.accel, sample.dt);
        let a_len = acc.length();
        if !self.started {
            if a_len <= 0.0 || !a_len.is_finite() {
                return;
            }
            self.q = arc(acc / a_len, WORLD_UP);
            self.gravity = a_len;
            self.started = true;
        }
        let rate = gyro - self.bias;
        self.rate = rate;
        // At rest the reading is bias; learn it slowly.
        let near_g = (a_len - self.gravity).abs() < STILL_ACCEL * self.gravity;
        if near_g && rate.length() < STILL_RATE {
            self.still += dt;
            if self.still > STILL_SECONDS {
                let tau = if self.bias_seconds < BIAS_WARMUP { BIAS_TAU_START } else { BIAS_TAU };
                self.bias_seconds += dt;
                let k = (dt / tau).min(1.0);
                self.bias += (gyro - self.bias) * k;
                self.gravity += (a_len - self.gravity) * k;
            }
        } else {
            self.still = 0.0;
        }
        // Pull the estimated up towards the measured one (Mahony).
        let mut corrected = rate;
        if a_len > 0.5 * self.gravity && a_len < 1.5 * self.gravity {
            let up = self.q.invert().rotate_vec3(&WORLD_UP);
            let error = Vec3f::cross(acc / a_len, up);
            let gain = if self.elapsed < START_SECONDS { GRAVITY_GAIN_START } else { GRAVITY_GAIN };
            corrected += error * gain;
        }
        self.q = integrate_body(self.q, corrected, dt);
        if !self.q.is_finite() {
            let yaw_ref = self.yaw_ref;
            *self = Self { yaw_ref, ..Self::default() };
            return;
        }
        self.elapsed += dt;
    }
}

/// `q ⊗ exp(ω·dt/2)`: rotate `q` by a head-frame rate over `dt`.
fn integrate_body(q: Quat, rate: Vec3f, dt: f32) -> Quat {
    let speed = rate.length();
    let angle = speed * dt;
    if angle <= 0.0 || !angle.is_finite() {
        return q;
    }
    let step = Quat::from_axis_angle(rate / speed, angle);
    // `Quat::multiply(a, b)` is the Hamilton product b ⊗ a.
    Quat::multiply(&step, &q).normalized()
}

/// The rotation about world up that turns forward (-z) to the head's
/// heading.
fn heading(q: Quat) -> Quat {
    let forward = q.rotate_vec3(&Vec3f { x: 0.0, y: 0.0, z: -1.0 });
    let mut h = Vec3f { x: forward.x, y: 0.0, z: forward.z };
    if h.length_squared() < 0.01 {
        // Looking straight down or up: the top of the head points forward
        // (down) or back (up).
        let up = q.rotate_vec3(&WORLD_UP);
        let s = if forward.y < 0.0 { 1.0 } else { -1.0 };
        h = Vec3f { x: up.x * s, y: 0.0, z: up.z * s };
    }
    if h.length_squared() < 1e-6 {
        return Quat::default();
    }
    Quat::from_axis_angle(WORLD_UP, f32::atan2(-h.x, -h.z))
}

/// The rotation taking unit `from` onto unit `to`.
fn arc(from: Vec3f, to: Vec3f) -> Quat {
    let d = from.dot(to);
    if d < -0.9999 {
        let other = if from.x.abs() < 0.9 { Vec3f { x: 1.0, y: 0.0, z: 0.0 } } else { Vec3f { x: 0.0, y: 0.0, z: 1.0 } };
        return Quat::from_axis_angle(Vec3f::cross(from, other).normalize(), std::f32::consts::PI);
    }
    let c = Vec3f::cross(from, to);
    Quat { x: c.x, y: c.y, z: c.z, w: 1.0 + d }.normalized()
}
