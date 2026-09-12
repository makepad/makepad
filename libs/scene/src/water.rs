//! Published water surfaces and the shared wave coefficients.
use makepad_math::*;

/// One analytic wave component. All fields are the RAW coefficients the
/// canonical expression consumes — the render path uploads them unmodified
/// (no unit conversion that could drift between CPU and GPU).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterWave {
    /// Unit travel direction on the ground plane.
    pub dir_x: f32,
    pub dir_z: f32,
    /// Amplitude, world units (crest = level + amp for a steady wave).
    pub amp: f32,
    /// Spatial frequency k = τ / wavelength.
    pub k: f32,
    /// Temporal frequency ω = k · phase-speed.
    pub omega: f32,
    /// Phase offset, radians.
    pub phase: f32,
    /// 0 = steady. ≥ 1 = set-wave train: the envelope `(0.5+0.5·cos(phase/
    /// group))²` makes every `group`-th crest the big one (surf sets).
    pub group: f32,
}

impl WaterWave {
    /// Build from the authorable quantities: direction (normalized here),
    /// amplitude, wavelength and phase speed.
    pub fn new(dir_x: f32, dir_z: f32, amp: f32, len: f32, speed: f32) -> Self {
        let l = makepad_math::deterministic::hypot(dir_x, dir_z);
        let (dx, dz) = if l > 1.0e-6 {
            (dir_x / l, dir_z / l)
        } else {
            (1.0, 0.0)
        };
        let k = std::f32::consts::TAU / len.max(0.05);
        Self {
            dir_x: dx,
            dir_z: dz,
            amp: amp.max(0.0),
            k,
            omega: k * speed.max(0.0),
            phase: 0.0,
            group: 0.0,
        }
    }
}

/// Wave slots the render sheet mirrors per volume. The sim accepts no more
/// either (the verbs warn and drop extras), so physics and visuals can never
/// disagree about which waves exist.
pub const MAX_WAVES: usize = 8;

/// A visible water region, with its still surface at `max.y`.
#[derive(Clone, Debug)]
pub struct WaterSurface {
    pub min: Vec3f,
    pub max: Vec3f,
    pub waves: Vec<WaterWave>,
    pub color: Vec4f,
    pub entity: u64,
    pub draw_sheet: bool,
}
impl WaterSurface {
    pub fn level(&self) -> f32 { self.max.y }
    pub fn amp_sum(&self) -> f32 { self.waves.iter().map(|w| w.amp).sum() }
}
#[derive(Clone, Debug, Default)]
pub struct WaterView {
    pub rev: u64,
    pub volumes: Vec<WaterSurface>,
}

/// Wave-sum time for a tick. f32 on purpose: the shader receives the same
/// f32, so both sides reduce identically. Accuracy (not determinism) of the
/// sum degrades as ω·t grows — days of continuous runtime; documented.
#[inline]
pub fn tick_time(tick: u64, dt: f32) -> f32 {
    tick as f32 * dt
}

