//! Head orientation (3DOF) from a headset's IMU, for any IMU device.
//!
//! Two layers below the view's pose sources (`stereo::tracking`):
//!
//! * A backend ([`ImuBackend`]) owns one kind of device and its protocol,
//!   and yields [`ImuSample`]s already in the head frame and SI units with
//!   the step from the device's own clock. Today: the Valve IMU inside a
//!   Bigscreen Beyond ([`valve`]) and the XREAL One Pro ([`xreal`]). Another
//!   headset is one more backend in [`BACKENDS`].
//! * [`fusion`] turns samples into an orientation, the same for every
//!   backend.
//!
//! [`HeadsetImu`] runs both on one long-lived worker: it opens the first
//! backend that finds its device, reads it until it goes away, then
//! searches again. The UI reads the newest pose through a sequence lock
//! and never waits on the worker.
//!
//! Orientation only: nothing here derives a position. Without an absolute
//! heading reference pitch and roll are held to gravity and yaw is
//! integrated gyro, which drifts (slowly once the at-rest bias estimate has
//! settled, but without bound); [`HeadsetImu::recenter`] makes the current
//! heading forward.
pub mod fusion;
#[cfg(target_os = "linux")]
pub mod valve;
#[cfg(target_os = "linux")]
pub mod xreal;

use {
    fusion::{ImuSample, OrientationFilter},
    makepad_widgets::{
        makepad_platform::thread::{TaskHandle, ThreadOptions, ThreadSpawner},
        Cx, Quat, Vec3f,
    },
    std::{
        cell::Cell,
        path::PathBuf,
        sync::{
            atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
            Arc, Mutex,
        },
    },
};

/// Environment variable naming the IMU's device node (a hidraw path),
/// bypassing discovery.
pub const HEADSET_IMU_ENV: &str = "MAKEPAD_HEADSET_IMU_HID";

/// The furthest a pose is extrapolated past its sample: a stalled stream
/// holds its last pose instead of spinning on its last rate.
const PREDICT_MAX: f64 = 0.05;
/// How long a backend's `pump` may wait per call, so `stop` is seen
/// promptly.
const PUMP_MS: i32 = 250;
/// No sample for this long reads as stalled (headset off or asleep); the
/// device stays open.
const STALL_SECONDS: f64 = 2.0;
/// The worker recenters once by itself this long after the first sample,
/// when the start-up gravity fix has settled.
const AUTO_RECENTER_SECONDS: f64 = 1.0;

/// Which device to read.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadsetImuSource {
    /// Every backend searches for its own device; `MAKEPAD_HEADSET_IMU_HID`
    /// overrides.
    Auto,
    /// Match the headset whose display the VR view is using. A backend may
    /// configure stereo on this display; preview/Auto only reads tracking.
    Display([u8; 3], u16),
    /// This device node, offered to every backend in turn (each checks it
    /// is its own device).
    Path(PathBuf),
}

/// One kind of IMU device: what it is and how it reads.
pub trait ImuBackend: Send {
    fn name(&self) -> &'static str;
    /// The node, serial and calibration source, for status and logs.
    fn describe(&self) -> String;
    /// Wait up to `timeout_ms` for data and deliver every new sample,
    /// oldest first. `Err` means the device is gone.
    fn pump(&mut self, timeout_ms: i32, out: &mut dyn FnMut(ImuSample)) -> Result<Pumped, String>;
    /// The headset display's optics, when this device's config carries
    /// them (read once at open).
    fn display_calibration(&self) -> Option<crate::stereo::calibration::DisplayCalibration> {
        None
    }
}

/// Result of [`ImuBackend::pump`].
pub enum Pumped {
    /// The device sent something (not necessarily a new sample).
    Data,
    /// Nothing arrived within the timeout.
    Idle,
}

/// Result of a backend's discovery.
pub enum Opened {
    Ready(Box<dyn ImuBackend>),
    /// No device of this kind.
    NotFound,
    /// The device is there but cannot be used yet (access, calibration);
    /// the reason is shown and discovery repeats.
    Unavailable(String),
}

/// One backend: its name and how it finds and opens its device.
pub struct BackendKind {
    pub name: &'static str,
    pub open: fn(&HeadsetImuSource) -> Opened,
}

/// The IMU backends this build has, in the order they are tried.
pub const BACKENDS: &[BackendKind] = &[
    #[cfg(target_os = "linux")]
    valve::BACKEND,
    #[cfg(target_os = "linux")]
    xreal::BACKEND,
];

/// The newest fused orientation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadPose {
    source_generation: u32,
    /// Head-to-world rotation. World is +y up (gravity), +x right, -z
    /// forward at the last recenter; the head frame is the same axes as
    /// seen from the wearer.
    pub orientation: Quat,
    /// Head-frame angular velocity, rad/s, bias removed.
    pub angular_velocity: Vec3f,
    /// `Cx::monotonic_now()` when the sample arrived.
    pub time: f64,
    /// Samples fused since the device was opened.
    pub samples: u64,
}

impl HeadPose {
    /// The orientation extrapolated to `now` (`Cx::monotonic_now()`, e.g.
    /// the expected present time) on the last angular velocity, at most
    /// `PREDICT_MAX` past the sample.
    pub fn predicted(&self, now: f64) -> Quat {
        let dt = (now - self.time).clamp(0.0, PREDICT_MAX) as f32;
        let w = self.angular_velocity;
        let speed = w.length();
        if speed * dt <= 0.0 || !speed.is_finite() {
            return self.orientation;
        }
        // q ⊗ exp(ω·dt/2); `Quat::multiply(a, b)` is b ⊗ a.
        let step = Quat::from_axis_angle(w / speed, speed * dt);
        Quat::multiply(&step, &self.orientation).normalized()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadsetImuState {
    /// No backend on this platform, or the worker could not start.
    Unsupported,
    /// No device found yet; discovery repeats every second.
    Searching,
    /// A device was found but cannot be used yet (see the detail).
    Unavailable,
    /// Open; waiting for samples and the first gravity fix.
    Starting,
    Tracking,
    /// Open, but no sample for a while.
    Stalled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HeadsetImuStatus {
    pub state: HeadsetImuState,
    /// The backend, node, calibration and the last problem, for logs and
    /// the UI.
    pub detail: String,
}

/// The UI's handle: start once, read [`Self::latest`] per frame.
pub struct HeadsetImu {
    shared: Arc<Shared>,
    worker: Option<TaskHandle<()>>,
    /// The last consistent read, returned if a read races the writer.
    last: Cell<Option<HeadPose>>,
}

impl HeadsetImu {
    pub fn start(spawner: &ThreadSpawner, source: HeadsetImuSource) -> Self {
        let shared = Arc::new(Shared { source: Mutex::new(source), ..Shared::default() });
        let mut worker = None;
        if BACKENDS.is_empty() {
            shared.set_status(HeadsetImuState::Unsupported, "no headset IMU backend on this platform".into());
        } else {
            shared.set_status(HeadsetImuState::Searching, "starting".into());
            let worker_shared = shared.clone();
            let options = ThreadOptions { name: Some("wm-headset-imu".into()), ..Default::default() };
            match spawner.spawn_worker(options, move || run(&worker_shared)) {
                Ok(handle) => worker = Some(handle),
                Err(error) => {
                    shared.set_status(HeadsetImuState::Unsupported, format!("worker did not start: {error}"))
                }
            }
        }
        Self { shared, worker, last: Cell::new(None) }
    }

    /// Retarget the existing worker after a display hotplug. The lock is
    /// held only for this small value, never while opening or reading a device.
    pub fn select_source(&self, source: HeadsetImuSource) {
        let mut selected = self.shared.source.lock().unwrap_or_else(|p| p.into_inner());
        if *selected == source { return; }
        *selected = source;
        self.shared.source_generation.fetch_add(1, Ordering::Release);
        self.shared.display_generation.fetch_add(1, Ordering::Release);
    }

    /// The newest pose, `None` until the first gravity fix. After the
    /// device stalls or goes away this stays the last pose; `time` and
    /// [`Self::status`] tell. Never waits: a read that overlaps a write (a
    /// few nanoseconds per sample) retries briefly and otherwise returns
    /// the previous pose.
    pub fn latest(&self) -> Option<HeadPose> {
        let generation = self.shared.source_generation.load(Ordering::Acquire);
        for _ in 0..64 {
            if let Some(pose) = self.shared.read() {
                if pose.source_generation != generation { self.last.set(None); return None; }
                if pose.samples > 0 {
                    self.last.set(Some(pose));
                }
                return self.last.get();
            }
            std::hint::spin_loop();
        }
        self.last.get().filter(|pose| pose.source_generation == generation)
    }

    /// Make the current heading forward. Pitch and roll stay on gravity.
    /// Applied by the worker at its next sample (about 1 ms). The worker
    /// also does this once by itself, a second after the device opens.
    pub fn recenter(&self) {
        self.shared.recenter.fetch_add(1, Ordering::AcqRel);
    }

    /// Bumps whenever [`Self::display_calibration`] may have changed (a
    /// device opened or went away). One atomic load: poll it per frame and
    /// fetch the calibration only on a change.
    pub fn display_generation(&self) -> u32 {
        self.shared.display_generation.load(Ordering::Acquire)
    }

    /// The open device's display optics, if its config has them. Takes a
    /// lock the worker holds only while opening or closing a device.
    pub fn display_calibration(&self) -> Option<Arc<crate::stereo::calibration::DisplayCalibration>> {
        match self.shared.display.lock() {
            Ok(slot) => slot.as_ref().filter(|(g, _)| *g == self.shared.source_generation.load(Ordering::Acquire)).map(|(_, c)| c.clone()),
            Err(poisoned) => poisoned.into_inner().as_ref().filter(|(g, _)| *g == self.shared.source_generation.load(Ordering::Acquire)).map(|(_, c)| c.clone()),
        }
    }

    pub fn status(&self) -> HeadsetImuStatus {
        let state = HeadsetImuState::from_u8(self.shared.state.load(Ordering::Acquire));
        let detail = match self.shared.detail.lock() {
            Ok(d) => d.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        HeadsetImuStatus { state, detail }
    }

    /// Stop the worker. Returns at once; reading stops within one pump
    /// (250 ms), followed by any device's mode-restoration I/O. Also on drop.
    pub fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.cancel();
            worker.detach();
        }
    }
}

impl Drop for HeadsetImu {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl HeadsetImuState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Searching,
            2 => Self::Unavailable,
            3 => Self::Starting,
            4 => Self::Tracking,
            5 => Self::Stalled,
            _ => Self::Unsupported,
        }
    }
}

// ======================================================================
// Published state: a sequence lock over plain atomics
// ======================================================================

/// orientation xyzw, angular velocity xyz.
const POSE_WORDS: usize = 7;

#[derive(Default)]
struct Shared {
    source: Mutex<HeadsetImuSource>,
    source_generation: AtomicU32,
    pose_generation: AtomicU32,
    seq: AtomicU64,
    words: [AtomicU32; POSE_WORDS],
    time: AtomicU64,
    samples: AtomicU64,
    state: AtomicU8,
    /// Written on transitions only, never per sample.
    detail: Mutex<String>,
    recenter: AtomicU32,
    stop: AtomicBool,
    /// The open device's display optics; replaced on open and close only,
    /// with `display_generation` bumped so the UI locks only on a change.
    display: Mutex<Option<(u32, Arc<crate::stereo::calibration::DisplayCalibration>)>>,
    display_generation: AtomicU32,
}

impl Shared {
    /// Single writer: the worker.
    fn write(&self, pose: &HeadPose) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let q = pose.orientation;
        let w = pose.angular_velocity;
        for (slot, v) in self.words.iter().zip([q.x, q.y, q.z, q.w, w.x, w.y, w.z]) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
        self.time.store(pose.time.to_bits(), Ordering::Relaxed);
        self.samples.store(pose.samples, Ordering::Relaxed);
        self.pose_generation.store(pose.source_generation, Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// `None` when the read overlapped a write.
    fn read(&self) -> Option<HeadPose> {
        let before = self.seq.load(Ordering::Acquire);
        if before & 1 != 0 {
            return None;
        }
        let mut v = [0f32; POSE_WORDS];
        for (out, slot) in v.iter_mut().zip(self.words.iter()) {
            *out = f32::from_bits(slot.load(Ordering::Relaxed));
        }
        let time = f64::from_bits(self.time.load(Ordering::Relaxed));
        let samples = self.samples.load(Ordering::Relaxed);
        let source_generation = self.pose_generation.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.seq.load(Ordering::Relaxed) != before {
            return None;
        }
        Some(HeadPose {
            source_generation,
            orientation: Quat { x: v[0], y: v[1], z: v[2], w: v[3] },
            angular_velocity: Vec3f { x: v[4], y: v[5], z: v[6] },
            time,
            samples,
        })
    }

    fn set_status(&self, state: HeadsetImuState, detail: String) {
        let mut slot = match self.detail.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if *slot != detail || self.state.load(Ordering::Relaxed) != state as u8 {
            makepad_widgets::log!("headset imu: {state:?} {detail}");
        }
        *slot = detail;
        self.state.store(state as u8, Ordering::Release);
    }

    fn set_display(&self, display: Option<crate::stereo::calibration::DisplayCalibration>, generation: u32) {
        let mut slot = match self.display.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = display.map(|c| (generation, Arc::new(c)));
        self.display_generation.fetch_add(1, Ordering::AcqRel);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn sleep(&self, seconds: f64) {
        let end = Cx::monotonic_now() + seconds;
        while !self.stopped() && Cx::monotonic_now() < end {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

// ======================================================================
// The worker
// ======================================================================

impl Default for HeadsetImuSource {
    fn default() -> Self { Self::Auto }
}

fn run(shared: &Shared) {
    while !shared.stopped() {
        let (generation, source) = {
            let source = shared.source.lock().unwrap_or_else(|p| p.into_inner());
            (shared.source_generation.load(Ordering::Acquire), source.clone())
        };
        let source = match (&source, std::env::var_os(HEADSET_IMU_ENV)) {
            (HeadsetImuSource::Auto, Some(path)) => HeadsetImuSource::Path(PathBuf::from(path)),
            (HeadsetImuSource::Display(vendor, 0x1234), Some(path)) if vendor == b"BIG" => {
                HeadsetImuSource::Path(PathBuf::from(path))
            }
            _ => source,
        };
        let mut reasons = Vec::new();
        let mut backend = None;
        for kind in BACKENDS {
            match (kind.open)(&source) {
                Opened::Ready(b) => {
                    backend = Some(b);
                    break;
                }
                Opened::NotFound => {}
                Opened::Unavailable(reason) => reasons.push(format!("{}: {reason}", kind.name)),
            }
        }
        let Some(mut backend) = backend else {
            if reasons.is_empty() {
                shared.set_status(HeadsetImuState::Searching, "no headset IMU found".into());
            } else {
                shared.set_status(HeadsetImuState::Unavailable, reasons.join("; "));
            }
            shared.sleep(1.0);
            continue;
        };
        if shared.stopped() || shared.source_generation.load(Ordering::Acquire) != generation {
            continue;
        }
        let name = backend.name();
        shared.set_status(HeadsetImuState::Starting, format!("{name} {}", backend.describe()));
        shared.set_display(backend.display_calibration(), generation);
        let followed = follow(shared, backend.as_mut(), generation);
        shared.set_display(None, generation);
        // Restore a device's display mode before opening the next backend.
        // This runs on the worker, including on shutdown.
        drop(backend);
        if let Err(error) = followed {
            shared.set_status(HeadsetImuState::Searching, format!("{name}: {error}; searching again"));
            shared.sleep(1.0);
        }
    }
}

/// Read one backend until stop (Ok) or until its device goes away (Err).
fn follow(shared: &Shared, backend: &mut dyn ImuBackend, generation: u32) -> Result<(), String> {
    let name = backend.name();
    let mut filter = OrientationFilter::default();
    let mut recenter_seen = shared.recenter.load(Ordering::Acquire);
    let mut samples = 0u64;
    let mut first: Option<f64> = None;
    let mut auto_recentered = false;
    let mut last_data = Cx::monotonic_now();
    let mut state = HeadsetImuState::Starting;
    while !shared.stopped() && shared.source_generation.load(Ordering::Acquire) == generation {
        let mut fused = 0u64;
        backend.pump(PUMP_MS, &mut |sample| {
            filter.update(&sample);
            if filter.started() {
                fused += 1;
            }
        })?;
        let now = Cx::monotonic_now();
        if fused == 0 {
            // Magnetometer/status packets are not fresh orientation data.
            if state != HeadsetImuState::Stalled && now - last_data > STALL_SECONDS {
                state = HeadsetImuState::Stalled;
                shared.set_status(state, format!("{name} {}; no samples (headset off or asleep?)", backend.describe()));
            }
            continue;
        }
        last_data = now;
        samples += fused;
        let first = *first.get_or_insert(now);
        let recenter = shared.recenter.load(Ordering::Acquire);
        if recenter != recenter_seen || (!auto_recentered && now - first >= AUTO_RECENTER_SECONDS) {
            recenter_seen = recenter;
            auto_recentered = true;
            filter.recenter();
        }
        shared.write(&HeadPose { source_generation: generation, orientation: filter.orientation(), angular_velocity: filter.rate(), time: now, samples });
        if state != HeadsetImuState::Tracking {
            state = HeadsetImuState::Tracking;
            shared.set_status(state, format!("{name} {}; orientation only, yaw drifts: recenter resets the heading", backend.describe()));
        }
    }
    Ok(())
}
