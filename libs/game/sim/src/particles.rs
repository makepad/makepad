//! The particle *request* vocabulary — shared between the script layer that
//! produces requests and the renderer that consumes them.
//!
//! Nothing here is ever simulated by this crate. Particles are tier-3 Local
//! (game.md): they live in the renderer, on the device, with their own RNG.
//! These types exist in `sim` only because it is the crate both sides can
//! see; `GameWorld` has no particle field and `step_world` has no particle
//! code, which is what makes it structurally impossible for a particle to
//! advance the world RNG and break tape parity.

use makepad_math::*;

/// What a particle looks and behaves like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    /// Bright, fast, gravity-bound, short-lived: impacts and collisions.
    Spark,
    /// Slow, rising, expanding, soft fade: exhaust and fire.
    Smoke,
    /// Low, drifting, settles: wheels on dirt, footfalls.
    Dust,
    /// Near-stationary marker that fades in place: speed trails.
    Trail,
}

impl ParticleKind {
    pub fn parse(name: &str) -> ParticleKind {
        match name {
            "smoke" => ParticleKind::Smoke,
            "dust" => ParticleKind::Dust,
            "trail" => ParticleKind::Trail,
            _ => ParticleKind::Spark,
        }
    }

    /// Per-kind defaults: (life seconds, size, speed, gravity scale, drag).
    pub fn defaults(self) -> (f32, f32, f32, f32, f32) {
        match self {
            ParticleKind::Spark => (0.5, 0.06, 6.0, 1.0, 0.2),
            ParticleKind::Smoke => (1.4, 0.18, 1.2, -0.15, 1.4),
            ParticleKind::Dust => (0.9, 0.12, 1.6, 0.25, 2.0),
            ParticleKind::Trail => (0.6, 0.09, 0.2, 0.0, 3.0),
        }
    }
}

/// How an emitter is anchored: to a moving entity, or to a fixed point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmitterAnchor {
    Entity(u64),
    Point(Vec3f),
}

/// Tuning for one emitter or burst, as the script gave it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleSpec {
    pub kind: ParticleKind,
    /// Particles per second (emitters) or total count (bursts).
    pub rate: f32,
    pub life: f32,
    pub size: f32,
    pub color: Vec4f,
    /// Cone half-width of the launch direction, 0 = straight up.
    pub spread: f32,
    pub speed: f32,
    /// Multiplier on the renderer's particle gravity for this kind.
    pub gravity: f32,
}

impl ParticleSpec {
    pub fn new(kind: ParticleKind) -> Self {
        let (life, size, speed, gravity, _) = kind.defaults();
        Self {
            kind,
            rate: 24.0,
            life,
            size,
            color: vec4f(1.0, 0.85, 0.5, 1.0),
            spread: 0.5,
            speed,
            gravity,
        }
    }
}

/// A request from script, drained by the host each frame. Mirrors the
/// AudioRequest pattern: script queues, the device decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParticleRequest {
    /// Continuous emitter; `id` lets script stop or replace it.
    Emitter {
        id: u64,
        anchor: EmitterAnchor,
        spec: ParticleSpec,
    },
    /// One-shot puff of `spec.rate` particles.
    Burst { at: Vec3f, spec: ParticleSpec },
    Stop { id: u64 },
    Clear,
}
