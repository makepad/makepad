//! Device-local particles (game.md tier 3: Local).
//!
//! Particles are cosmetic, so they are *not* simulation state: they live
//! here, in the renderer, and the sim has no field for them and no code
//! that steps them. Script asks for them through a drain queue (the same
//! shape the audio verbs use), the host hands the requests to this system,
//! and this system owns everything after that — including its own RNG.
//!
//! That is the whole rng-isolation argument, and it is structural rather
//! than disciplinary: `GameWorld` cannot advance its RNG for a particle
//! because `GameWorld` never sees one. A device may run fewer particles
//! than its peers (a Quest cap vs a PC cap) with no risk of divergence,
//! because there is nothing to diverge.

use makepad_draw::*;

pub use makepad_game_sim::{EmitterAnchor, ParticleKind, ParticleRequest, ParticleSpec};

#[derive(Clone, Copy)]
struct Particle {
    pos: Vec3f,
    vel: Vec3f,
    age: f32,
    life: f32,
    size: f32,
    color: Vec4f,
    gravity: f32,
    drag: f32,
}

struct Emitter {
    id: u64,
    anchor: EmitterAnchor,
    spec: ParticleSpec,
    /// Fractional particle carried between frames so low rates still emit.
    accum: f32,
}

/// One instance to draw: a small camera-facing-ish cube, batched with the
/// alpha pass so it costs no extra draw call.
#[derive(Clone, Copy, Debug)]
pub struct ParticleInstance {
    pub pos: Vec3f,
    pub size: f32,
    pub color: Vec4f,
}

/// Device-local particle simulation. Deliberately not `Clone` and not part
/// of any snapshot: this state is per-device and must never be replicated.
pub struct ParticleSystem {
    particles: Vec<Particle>,
    emitters: Vec<Emitter>,
    /// Device-local RNG. NOT the world RNG — see the module docs.
    rng: u64,
    cap: usize,
    gravity: f32,
}

/// Desktop-class cap; standalone XR should lower it (see BUDGETS.md).
pub const DEFAULT_PARTICLE_CAP: usize = 2000;

impl Default for ParticleSystem {
    fn default() -> Self {
        Self::new(DEFAULT_PARTICLE_CAP)
    }
}

impl ParticleSystem {
    pub fn new(cap: usize) -> Self {
        Self {
            particles: Vec::new(),
            emitters: Vec::new(),
            // Fixed seed: a device's particles are reproducible for tests,
            // and nothing outside this device can observe the stream.
            rng: 0x9E37_79B9_7F4A_7C15,
            cap,
            gravity: 22.0,
        }
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
        self.particles.truncate(cap);
    }

    pub fn live_count(&self) -> usize {
        self.particles.len()
    }

    pub fn emitter_count(&self) -> usize {
        self.emitters.len()
    }

    /// Retire every local effect at a whole-world boundary while preserving
    /// this device's particle cap. Entity-anchored emitters carry realm-local
    /// ids, so allowing them across a replacement could attach old smoke to
    /// an unrelated entity that reused the same id.
    pub fn clear(&mut self) {
        self.emitters.clear();
        self.particles.clear();
    }

    fn next_f32(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 * (1.0 / 16777216.0)
    }

    fn signed(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    /// Apply script's requests. Called by the host with whatever it drained.
    pub fn apply(&mut self, requests: &[ParticleRequest]) {
        for r in requests {
            match *r {
                ParticleRequest::Emitter { id, anchor, spec } => {
                    if let Some(e) = self.emitters.iter_mut().find(|e| e.id == id) {
                        e.anchor = anchor;
                        e.spec = spec;
                    } else {
                        self.emitters.push(Emitter {
                            id,
                            anchor,
                            spec,
                            accum: 0.0,
                        });
                    }
                }
                ParticleRequest::Burst { at, spec } => {
                    let n = spec.rate.max(0.0) as usize;
                    for _ in 0..n {
                        self.spawn(at, &spec);
                    }
                }
                ParticleRequest::Stop { id } => self.emitters.retain(|e| e.id != id),
                ParticleRequest::Clear => {
                    self.clear();
                }
            }
        }
    }

    fn spawn(&mut self, at: Vec3f, spec: &ParticleSpec) {
        if self.particles.len() >= self.cap {
            return;
        }
        let (_, _, _, _, drag) = spec.kind.defaults();
        let (sx, sy, sz) = (self.signed(), self.next_f32(), self.signed());
        let dir = vec3f(sx * spec.spread, 1.0 - spec.spread * 0.3 + sy * 0.3, sz * spec.spread);
        let jitter = 0.7 + self.next_f32() * 0.6;
        let life = spec.life * (0.75 + self.next_f32() * 0.5);
        self.particles.push(Particle {
            pos: at,
            vel: dir.normalize() * spec.speed * jitter,
            age: 0.0,
            life,
            size: spec.size,
            color: spec.color,
            gravity: spec.gravity,
            drag,
        });
    }

    /// Step every emitter and particle. `entity_pos` resolves entity-anchored
    /// emitters — the host passes a lookup into the world it is drawing, so
    /// particles follow objects without the sim carrying particle state.
    pub fn step(&mut self, dt: f32, entity_pos: &dyn Fn(u64) -> Option<Vec3f>) {
        // Emit.
        let mut pending: Vec<(Vec3f, ParticleSpec, usize)> = Vec::new();
        for e in self.emitters.iter_mut() {
            let Some(at) = (match e.anchor {
                EmitterAnchor::Point(p) => Some(p),
                EmitterAnchor::Entity(id) => entity_pos(id),
            }) else {
                continue;
            };
            e.accum += e.spec.rate * dt;
            let n = e.accum.floor().max(0.0);
            e.accum -= n;
            if n > 0.0 {
                pending.push((at, e.spec, n as usize));
            }
        }
        // Entity-anchored emitters whose entity is gone stop emitting; drop
        // them so a long game does not accumulate dead emitters.
        self.emitters.retain(|e| match e.anchor {
            EmitterAnchor::Entity(id) => entity_pos(id).is_some(),
            EmitterAnchor::Point(_) => true,
        });
        for (at, spec, n) in pending {
            for _ in 0..n {
                self.spawn(at, &spec);
            }
        }
        // Integrate.
        let gravity = self.gravity;
        for p in self.particles.iter_mut() {
            p.age += dt;
            p.vel.y -= gravity * p.gravity * dt;
            let damp = 1.0 - (p.drag * dt).min(1.0);
            p.vel = p.vel * damp;
            p.pos = p.pos + p.vel * dt;
        }
        self.particles.retain(|p| p.age < p.life);
    }

    /// Instances for this frame, already faded. Drawn by the alpha batch.
    pub fn instances(&self) -> Vec<ParticleInstance> {
        self.particles
            .iter()
            .map(|p| {
                let t = (p.age / p.life).clamp(0.0, 1.0);
                let fade = 1.0 - t;
                // Smoke grows as it fades; sparks shrink.
                let grow = if p.gravity < 0.0 { 1.0 + t * 1.5 } else { 1.0 - t * 0.4 };
                ParticleInstance {
                    pos: p.pos,
                    size: p.size * grow,
                    color: vec4f(p.color.x, p.color.y, p.color.z, p.color.w * fade),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_entities(_: u64) -> Option<Vec3f> {
        None
    }

    fn spec(kind: ParticleKind) -> ParticleSpec {
        ParticleSpec::new(kind)
    }

    #[test]
    fn burst_spawns_exactly_its_count() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Spark);
        s.rate = 12.0;
        ps.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 1.0, 0.0),
            spec: s,
        }]);
        assert_eq!(ps.live_count(), 12);
    }

    #[test]
    fn emitter_rate_accumulates_across_frames() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Smoke);
        s.rate = 10.0;
        ps.apply(&[ParticleRequest::Emitter {
            id: 1,
            anchor: EmitterAnchor::Point(vec3f(0.0, 0.0, 0.0)),
            spec: s,
        }]);
        // 10/s over 6 frames of 1/60 = 1.0 particle.
        for _ in 0..6 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        assert_eq!(ps.live_count(), 1);
        for _ in 0..54 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        // A full second: 10 emitted, none dead yet (smoke lives ~1.4s).
        assert_eq!(ps.live_count(), 10);
    }

    #[test]
    fn the_cap_is_enforced_and_lowering_it_truncates() {
        let mut ps = ParticleSystem::new(32);
        let mut s = spec(ParticleKind::Spark);
        s.rate = 500.0;
        ps.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 0.0, 0.0),
            spec: s,
        }]);
        assert_eq!(ps.live_count(), 32, "burst must not exceed the cap");
        ps.set_cap(8);
        assert_eq!(ps.live_count(), 8);
    }

    #[test]
    fn particles_expire() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Spark);
        s.rate = 5.0;
        s.life = 0.25;
        ps.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 5.0, 0.0),
            spec: s,
        }]);
        assert_eq!(ps.live_count(), 5);
        for _ in 0..60 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        assert_eq!(ps.live_count(), 0, "all should have aged out");
    }

    #[test]
    fn entity_anchored_emitters_follow_and_then_retire() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Dust);
        s.rate = 60.0;
        ps.apply(&[ParticleRequest::Emitter {
            id: 7,
            anchor: EmitterAnchor::Entity(42),
            spec: s,
        }]);
        let at_x10 = |id: u64| (id == 42).then_some(vec3f(10.0, 0.0, 0.0));
        ps.step(1.0 / 60.0, &at_x10);
        assert_eq!(ps.live_count(), 1);
        assert!((ps.instances()[0].pos.x - 10.0).abs() < 0.5);
        // Entity gone: the emitter retires rather than lingering forever.
        ps.step(1.0 / 60.0, &no_entities);
        assert_eq!(ps.emitter_count(), 0);
    }

    #[test]
    fn stop_and_clear() {
        let mut ps = ParticleSystem::default();
        let s = spec(ParticleKind::Smoke);
        ps.apply(&[ParticleRequest::Emitter {
            id: 3,
            anchor: EmitterAnchor::Point(vec3f(0.0, 0.0, 0.0)),
            spec: s,
        }]);
        assert_eq!(ps.emitter_count(), 1);
        ps.apply(&[ParticleRequest::Stop { id: 3 }]);
        assert_eq!(ps.emitter_count(), 0);
        let mut burst = s;
        burst.rate = 4.0;
        ps.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 0.0, 0.0),
            spec: burst,
        }]);
        assert_eq!(ps.live_count(), 4);
        ps.clear();
        assert_eq!(ps.live_count(), 0);
        assert_eq!(ps.emitter_count(), 0);
        assert_eq!(ps.cap(), DEFAULT_PARTICLE_CAP, "realm clear preserves device budget");
    }

    #[test]
    fn instances_fade_over_the_lifetime() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Spark);
        s.rate = 1.0;
        s.life = 1.0;
        s.color = vec4f(1.0, 1.0, 1.0, 1.0);
        ps.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 10.0, 0.0),
            spec: s,
        }]);
        let a0 = ps.instances()[0].color.w;
        for _ in 0..20 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        let a1 = ps.instances()[0].color.w;
        assert!(a1 < a0, "alpha should decay: {a0} -> {a1}");
    }

    #[test]
    fn smoke_rises_and_sparks_fall() {
        let mut ps = ParticleSystem::default();
        let mut smoke = spec(ParticleKind::Smoke);
        smoke.rate = 1.0;
        smoke.spread = 0.0;
        ps.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 0.0, 0.0),
            spec: smoke,
        }]);
        for _ in 0..30 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        assert!(ps.instances()[0].pos.y > 0.0, "smoke should rise");

        let mut ps2 = ParticleSystem::default();
        let mut sp = spec(ParticleKind::Spark);
        sp.rate = 1.0;
        sp.speed = 0.0;
        sp.spread = 0.0;
        ps2.apply(&[ParticleRequest::Burst {
            at: vec3f(0.0, 5.0, 0.0),
            spec: sp,
        }]);
        for _ in 0..20 {
            ps2.step(1.0 / 60.0, &no_entities);
        }
        assert!(ps2.instances()[0].pos.y < 5.0, "sparks should fall");
    }
}
