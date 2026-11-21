//! A world-tracked camera (`Cx::ar_start`): `Event::ArFrame` samples in native landscape sensor
//! orientation, `Event::ArState` for the session, raycasts through the newest image
//! (`Cx::ar_raycast`). Backed by the HAL's AR session (ARKit on iOS); elsewhere `Cx::ar_caps`
//! is empty and a start answers `ArSessionState::Failed(100)`.
use crate::makepad_math::{CameraFov, Mat4f, Pose, Vec3f};

/// What the device's world-tracked camera can do (`Cx::ar_caps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArCaps {
    pub world_tracking: bool,
    pub scene_depth: bool,
    /// The session can provide temporally smoothed scene depth.
    pub smoothed_depth: bool,
    /// The session can reconstruct a mesh of the physical surroundings.
    pub scene_mesh: bool,
}

/// Optional features of a world-tracked camera session. Unsupported requests are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArConfig {
    /// Request depth aligned with each camera picture, preferring smoothed depth.
    pub scene_depth: bool,
    /// Request reconstruction of the scene as a mesh for surface queries.
    pub scene_mesh: bool,
}

/// Why tracking is limited (ARKit's reasons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArTrackingReason {
    None,
    Initializing,
    ExcessiveMotion,
    InsufficientFeatures,
    Relocalizing,
    Other(u32),
}

/// How well the camera's pose is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArTracking {
    NotAvailable,
    Limited(ArTrackingReason),
    Normal,
}

/// One camera sample (`Event::ArFrame`); the newest replaces an undelivered one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArFrameEvent {
    /// Capture sequence, including captures skipped between deliveries.
    pub seq: u64,
    /// Capture time in nanoseconds on the host clock.
    pub time_ns: u64,
    pub tracking: ArTracking,
    /// Camera in session metres, y up and -z forward, with the long sensor side along x.
    pub camera: Pose,
    /// Left, right, up and down angles in radians. Left and down are negative.
    pub fov: [f32; 4],
    /// Focal lengths fx, fy and principal point cx, cy in sensor pixels.
    pub intrinsics: [f32; 4],
    /// Width and height of the native landscape sensor image in pixels.
    pub image_size: [u32; 2],
    /// Full-range biplanar 4:2:0 camera image with a BT.601 matrix, or zero if unavailable.
    /// The HAL keeps the pictures for the four newest capture sequences.
    pub picture: u64,
    /// Scene depth covering this camera's field of view, when available. Both views
    /// describe this camera. Zero near and far mean linear metres in .r, no projection.
    /// Import its image with `Cx::import_depth_image` within the picture's lifetime.
    #[cfg(not(target_arch = "wasm32"))]
    pub depth: Option<makepad_hal::XrDepth>,
    /// Handle of the corresponding UInt8 confidence map, or zero when unavailable.
    /// The codes are 0 (low), 1 (medium), 2 (high). It shares the picture's lifetime.
    pub depth_confidence: u64,
}

impl ArFrameEvent {
    /// Projection of the sensor image with the given near and far distances in metres.
    pub fn projection(&self, near: f32, far: f32) -> Mat4f {
        let [angle_left, angle_right, angle_up, angle_down] = self.fov;
        Mat4f::from_camera_fov(&CameraFov { angle_left, angle_right, angle_up, angle_down }, near, far)
    }
}

/// The world-tracked camera session's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArSessionState {
    Stopped,
    Running(ArTracking),
    Interrupted,
    /// ARKit error code. Code 100 means world tracking is unavailable here.
    Failed(i32),
}

/// A change of the session's state (`Event::ArState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArStateEvent {
    pub state: ArSessionState,
}

/// Where a raycast met a surface (`Cx::ar_raycast`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArHit {
    /// Surface point in session metres.
    pub position: Vec3f,
    /// Surface normal in session space.
    pub normal: Vec3f,
}

/// The system's thermal state (`Cx::thermal_state`), from cool to throttling hard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[cfg(not(target_arch = "wasm32"))]
impl ArCaps {
    pub(crate) fn from_hal(bits: u32) -> Self {
        Self {
            world_tracking: bits & makepad_hal::ar_caps::WORLD_TRACKING != 0,
            scene_depth: bits & makepad_hal::ar_caps::SCENE_DEPTH != 0,
            smoothed_depth: bits & makepad_hal::ar_caps::SMOOTHED_DEPTH != 0,
            scene_mesh: bits & makepad_hal::ar_caps::SCENE_MESH != 0,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ArHit {
    pub(crate) fn from_hal([x, y, z, nx, ny, nz]: [f32; 6]) -> Self {
        Self { position: Vec3f { x, y, z }, normal: Vec3f { x: nx, y: ny, z: nz } }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ThermalState {
    pub(crate) fn from_hal(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Nominal),
            1 => Some(Self::Fair),
            2 => Some(Self::Serious),
            3 => Some(Self::Critical),
            _ => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ArTracking {
    fn from_hal(bits: u32) -> Self {
        match bits & 0xff {
            makepad_hal::ar_tracking::NORMAL => Self::Normal,
            makepad_hal::ar_tracking::LIMITED => Self::Limited(match (bits >> 8) & 0xff {
                0 => ArTrackingReason::None,
                1 => ArTrackingReason::Initializing,
                2 => ArTrackingReason::ExcessiveMotion,
                3 => ArTrackingReason::InsufficientFeatures,
                4 => ArTrackingReason::Relocalizing,
                other => ArTrackingReason::Other(other),
            }),
            _ => Self::NotAvailable,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<makepad_hal::ArFrameInfo> for ArFrameEvent {
    fn from(frame: makepad_hal::ArFrameInfo) -> Self {
        let [x, y, z, w] = frame.camera.pose.orientation;
        let orientation = crate::makepad_math::Quat { x, y, z, w };
        let [x, y, z] = frame.camera.pose.position;
        Self {
            seq: frame.seq,
            time_ns: frame.time_ns,
            tracking: ArTracking::from_hal(frame.tracking),
            camera: Pose { orientation, position: Vec3f { x, y, z } },
            fov: frame.camera.fov,
            intrinsics: frame.intrinsics,
            image_size: frame.image,
            picture: frame.picture,
            depth: (frame.depth.layers != 0).then_some(frame.depth),
            depth_confidence: frame.depth_confidence,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ArStateEvent {
    pub(crate) fn from_hal(state: u32, code: i32) -> Option<Self> {
        use makepad_hal::ar_state;
        let state = match state {
            ar_state::STOPPED => ArSessionState::Stopped,
            ar_state::RUNNING => ArSessionState::Running(ArTracking::from_hal(code as u32)),
            ar_state::INTERRUPTED => ArSessionState::Interrupted,
            ar_state::FAILED => ArSessionState::Failed(code),
            _ => return None,
        };
        Some(Self { state })
    }
}
