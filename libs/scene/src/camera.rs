//! Camera description and effects resolved by the scene producer per view.
use makepad_math::*;

#[derive(Clone, Copy, Debug)]
pub struct CameraState {
    pub target: Vec3f,
    pub distance: f32,
    pub follow: u64,
    pub side: bool,
    pub third: u64,
    pub height: f32,
    pub boom: f32,
    pub fov: f32,
    pub near: f32,
}
impl Default for CameraState {
    fn default() -> Self {
        Self { target: vec3f(0.0, 2.0, 0.0), distance: 18.0, follow: 0, side: false,
            third: 0, height: 1.6, boom: 10.0, fov: 40.0, near: 0.15 }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraEffects {
    pub boom_limit: Option<f32>,
    pub shake_offset: Vec3f,
}

/// Decaying random camera offset (game.cam_shake). Hash of the tick, NOT the
/// world rng — pixels may wobble, simulation must not. Pinned to zero in tapes.
pub fn camera_shake_offset(tick: u64, amplitude: f32, distance: f32, in_test: bool) -> Vec3f {
    if in_test || amplitude <= 0.001 {
        return vec3f(0.0, 0.0, 0.0);
    }
    let mut h = tick.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    let fx = ((h & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    let fy = (((h >> 16) & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    let fz = (((h >> 32) & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    // The amplitude is world units of EYE offset, calibrated for a boomed
    // third-person camera. At first-person distances (boom ≈ 0.5) the same
    // offset swings the whole view — recoil at 6 rounds/s read as the camera
    // "spazzing out". Scale with the boom so a shake means the same thing at
    // every camera distance.
    let boom_scale = (distance / 8.0).clamp(0.12, 1.0);
    vec3f(fx, fy, fz) * amplitude * 0.35 * boom_scale
}

