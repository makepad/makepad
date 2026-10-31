//! Device-local particles and effects (game.md tier 3: Local).
//!
//! Particles are cosmetic, so they are *not* simulation state: they live
//! here, in the renderer, and the sim has no field for them and no code
//! that steps them. Script asks for them through a drain queue (the same
//! shape the audio verbs use), the host hands the requests to this system,
//! and this system owns everything after that — including its own RNG.
//!
//! That is the whole rng-isolation argument, and it is structural rather
//! than disciplinary: `World` cannot advance its RNG for a particle
//! because `World` never sees one. A device may run fewer particles
//! than its peers (a Quest budget vs a PC budget) with no risk of
//! divergence, because there is nothing to diverge.
//!
//! # GPU simulation
//!
//! The CPU never steps a particle. It keeps BURSTS: one record per layer of
//! an effect (or per frame of a running emitter), holding the launch
//! parameters and an age. The vertex shader (`vfx.rs`, `DrawSceneVfx`)
//! expands each record into its particles from a closed form of (index,
//! seed, age): ballistic flight under linear drag and gravity, stopped at
//! the ground, with size/colour/alpha over life, spin, wander and velocity
//! stretch. So the CPU cost is per burst, not per particle, and the upload
//! is one small record per burst per frame (see firework.rs for the same
//! argument). [`eval_particle`] is the same closed form in Rust: the CPU
//! path ([`ParticleSystem::cpu_instances`]) for backends that should not run
//! it in a vertex shader, and what the tests check.
//!
//! # Bounds
//!
//! `cap` is a soft budget of live particles: a burst that would pass it
//! evicts the OLDEST bursts first (mostly faded by then), so a new effect is
//! never the one that goes missing. Decals past their budget fade out early
//! instead of popping.

use makepad_draw::*;
use std::collections::HashMap;
use std::sync::Arc;

pub use makepad_scene::{
    vfx_default_dir, vfx_preset, vfx_preset_names, DecalRequest, EmitterAnchor, ParticleKind, ParticleRequest,
    ParticleSpec, VfxBlend, VfxDecal, VfxDecalKind, VfxLayer, VfxLight, VfxPreset, VfxPresetRef, VfxRequest,
    VfxSprite,
};

/// Quads per instance of the renderer's particle sheet: a burst larger than
/// this spans several instances (each with its own first index).
pub const PARTICLE_SHEET: usize = 32;

/// Instance flag bits (`kind.w`).
pub const VFX_COLLIDE: f32 = 1.0;
pub const VFX_FLAT: f32 = 2.0;
pub const VFX_EXPLICIT: f32 = 4.0;

/// One GPU record: a burst of particles (expanded in the vertex shader), or
/// with [`VFX_EXPLICIT`] one already-evaluated particle. Twelve vec4s, the
/// instance layout of `DrawSceneVfx`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct ParticleInstance {
    /// xyz origin, w age (seconds since the burst).
    pub origin: Vec4f,
    /// xyz launch axis (unit), w cone half-angle. Explicit: xyz velocity.
    pub dir: Vec4f,
    /// xyz emitter velocity along its path (births drift with it), w the
    /// seconds births are spread over.
    pub path: Vec4f,
    /// xyz velocity every particle inherits, w start delay. Explicit: w =
    /// rotation.
    pub inherit: Vec4f,
    /// life lo, life hi, speed lo, speed hi.
    pub life: Vec4f,
    /// size at birth, size at death, gravity (m/s² down), drag (1/s).
    pub size: Vec4f,
    pub color: Vec4f,
    pub color_end: Vec4f,
    /// emissive intensity, lit share, stretch seconds, spin rad/s.
    pub look: Vec4f,
    /// sprite id, blend (1 = additive), seed, flags.
    pub kind: Vec4f,
    /// spawn radius, soft fade metres, ground y, first particle index.
    pub extra: Vec4f,
    /// particle count, fade-in share, wobble metres, unused.
    pub extra2: Vec4f,
}

impl ParticleInstance {
    /// A single self-lit glowing dot (mocap markers, debug points).
    pub fn marker(pos: Vec3f, size: f32, color: Vec4f) -> Self {
        Self {
            origin: vec4(pos.x, pos.y, pos.z, 0.0),
            life: vec4(1.0, 1.0, 0.0, 0.0),
            size: vec4(size, size, 0.0, 0.0),
            color,
            color_end: color,
            look: vec4(1.5, 0.0, 0.0, 0.0),
            kind: vec4(VfxSprite::Dot.id(), 1.0, 0.0, VFX_EXPLICIT),
            extra: vec4(0.0, 0.05, -1.0e9, 0.0),
            extra2: vec4(1.0, 0.0, 0.0, 0.0),
            ..Default::default()
        }
    }

    pub fn is_explicit(&self) -> bool {
        flag(self.kind.w, VFX_EXPLICIT)
    }

    pub fn additive(&self) -> bool {
        self.kind.y > 0.5
    }

    pub fn count(&self) -> usize {
        self.extra2.x.max(0.0) as usize
    }

    /// Seconds after which no particle of this record is alive.
    pub fn end(&self) -> f32 {
        self.inherit.w + self.path.w + self.life.y
    }
}

fn flag(flags: f32, bit: f32) -> bool {
    ((flags / bit).floor() as i32) & 1 == 1
}

/// One evaluated particle (the vertex shader's closed form, on the CPU).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvaluatedParticle {
    pub pos: Vec3f,
    pub vel: Vec3f,
    pub size: f32,
    pub color: Vec4f,
    pub rotation: f32,
    /// 0..1 through its life.
    pub life_t: f32,
    pub age: f32,
    pub life: f32,
}

fn hash(h: f32, k: f32, m: f32) -> f32 {
    let v = (h * k).sin() * m;
    v - v.floor()
}

/// The per-particle randoms, as the shader computes them.
pub fn particle_randoms(idx: f32, seed: f32) -> [f32; 6] {
    let h = idx * 0.618_034 + seed * 0.754_877_7;
    [
        hash(h, 12.9898, 43758.547),
        hash(h, 78.233, 24634.635),
        hash(h, 39.4257, 15731.743),
        hash(h, 26.651, 29417.23),
        hash(h, 91.117, 35729.91),
        hash(h, 53.791, 19541.37),
    ]
}

fn basis(dir: Vec3f) -> (Vec3f, Vec3f) {
    let hint = if dir.y.abs() < 0.99 { vec3f(0.0, 1.0, 0.0) } else { vec3f(1.0, 0.0, 0.0) };
    let t1 = Vec3f::cross(hint, dir).normalize();
    let t2 = Vec3f::cross(dir, t1);
    (t1, t2)
}

/// Particle `idx` of burst `r` at the record's age: the exact expression
/// `DrawSceneVfx`'s vertex shader evaluates. None while unborn or dead.
pub fn eval_particle(r: &ParticleInstance, idx: usize) -> Option<EvaluatedParticle> {
    let count = r.extra2.x.max(1.0);
    let fi = idx as f32;
    let [r1, r2, r3, r4, r5, r6] = particle_randoms(fi, r.kind.z);
    let birth = r.inherit.w + r.path.w * (fi + r1) / count;
    let age = r.origin.w - birth;
    let life = r.life.x + (r.life.y - r.life.x) * r2;
    if age < 0.0 || age > life || life <= 0.0 {
        return None;
    }
    let t = age / life;
    let axis = vec3f(r.dir.x, r.dir.y, r.dir.z);
    let (t1, t2) = basis(axis);
    let cos_t = 1.0 + (r.dir.w.cos() - 1.0) * r3;
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    let phi = r4 * std::f32::consts::TAU;
    let d = axis * cos_t + (t1 * phi.cos() + t2 * phi.sin()) * sin_t;
    let speed = r.life.z + (r.life.w - r.life.z) * r5;
    let inherit = vec3f(r.inherit.x, r.inherit.y, r.inherit.z);
    let v0 = d * speed + inherit;
    let jitter = vec3f(r6 * 2.0 - 1.0, r1 * 2.0 - 1.0, r3 * 2.0 - 1.0) * r.extra.x;
    let p0 = vec3f(r.origin.x, r.origin.y, r.origin.z)
        + vec3f(r.path.x, r.path.y, r.path.z) * (birth - r.inherit.w)
        + jitter;
    let k = r.size.w.max(0.001);
    let e = (-k * age).exp();
    let g = vec3f(0.0, -r.size.z, 0.0);
    let mut pos = p0 + v0 * ((1.0 - e) / k) + g * (age / k - (1.0 - e) / (k * k));
    let mut vel = v0 * e + g * ((1.0 - e) / k);
    let wobble = r.extra2.z;
    if wobble > 0.0 {
        pos += (t1 * (age * 2.7 + r2 * 6.283).sin() + t2 * (age * 2.1 + r4 * 6.283).cos()) * wobble;
    }
    if flag(r.kind.w, VFX_COLLIDE) && pos.y < r.extra.z {
        pos.y = r.extra.z + 0.01;
        vel.y = 0.0;
    }
    let size = (r.size.x + (r.size.y - r.size.x) * t) * (0.75 + 0.5 * r6);
    let mut color = r.color + (r.color_end - r.color) * t;
    if r.extra2.y > 0.0 {
        color.w *= (t / r.extra2.y).min(1.0);
    }
    let rotation = r2 * std::f32::consts::TAU + r.look.w * (r3 - 0.5) * 2.0 * age;
    Some(EvaluatedParticle { pos, vel, size, color, rotation, life_t: t, age, life })
}

struct Burst {
    rec: ParticleInstance,
    /// Ground not probed yet (resolved on the next step).
    ground_pending: bool,
}

struct Emitter {
    id: u64,
    anchor: EmitterAnchor,
    preset: Arc<VfxPreset>,
    params: Params,
    /// Fractional particle carried between frames per layer, so low rates
    /// still emit.
    accum: Vec<f32>,
    last: Option<Vec3f>,
    age: f32,
}

#[derive(Clone, Copy)]
struct Params {
    dir: Vec3f,
    scale: f32,
    color: Option<Vec4f>,
    intensity: f32,
    density: f32,
}

struct LightFlash {
    pos: Vec3f,
    color: Vec3f,
    radius: f32,
    life: f32,
    age: f32,
    /// Held (and flickering) while this emitter runs.
    emitter: u64,
}

/// One live surface mark, as the decal shader wants it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxDecalInstance {
    pub kind: VfxDecalKind,
    pub pos: Vec3f,
    pub normal: Vec3f,
    /// Unit in-plane axis the length runs along.
    pub dir: Vec3f,
    pub half_len: f32,
    pub half_width: f32,
    pub color: Vec4f,
    pub seed: f32,
    pub age: f32,
    pub life: f32,
}

impl VfxDecalInstance {
    /// 1 → 0 over the last fifth of its life.
    pub fn fade(&self) -> f32 {
        let t = (self.age / self.life.max(1.0e-3)).clamp(0.0, 1.0);
        ((1.0 - t) / 0.2).clamp(0.0, 1.0)
    }
}

/// Decals past this many fade out over a second, oldest first.
pub const DECAL_SOFT_BUDGET: usize = 4096;

/// Device-local particle simulation. Deliberately not `Clone` and not part
/// of any snapshot: this state is per-device and must never be replicated.
pub struct ParticleSystem {
    bursts: Vec<Burst>,
    emitters: Vec<Emitter>,
    lights: Vec<LightFlash>,
    decals: Vec<VfxDecalInstance>,
    /// Skid chains: (chain id, last point, time of it).
    chains: Vec<(u64, Vec3f, f32)>,
    /// Script-defined presets (`game.vfx_preset`), consulted before the
    /// built-in library.
    defined: HashMap<String, Arc<VfxPreset>>,
    /// Device-local RNG. NOT the world RNG — see the module docs.
    rng: u64,
    cap: usize,
    live: usize,
    time: f32,
    unknown_logged: Vec<String>,
}

/// Live-particle budget. The GPU expands bursts, so this is about fill
/// rate, not CPU; standalone XR should lower it (see BUDGETS.md).
pub const DEFAULT_PARTICLE_CAP: usize = 200_000;

impl Default for ParticleSystem {
    fn default() -> Self {
        Self::new(DEFAULT_PARTICLE_CAP)
    }
}

impl ParticleSystem {
    pub fn new(cap: usize) -> Self {
        Self {
            bursts: Vec::new(),
            emitters: Vec::new(),
            lights: Vec::new(),
            decals: Vec::new(),
            chains: Vec::new(),
            defined: HashMap::new(),
            // Fixed seed: a device's particles are reproducible for tests,
            // and nothing outside this device can observe the stream.
            rng: 0x9E37_79B9_7F4A_7C15,
            cap,
            live: 0,
            time: 0.0,
            unknown_logged: Vec::new(),
        }
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Lower (or raise) the live-particle budget; lowering evicts the oldest
    /// bursts and trims the next-oldest to fit.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap;
        self.make_room(0);
    }

    /// Particles in live bursts (a burst counts whole until it retires).
    pub fn live_count(&self) -> usize {
        self.live
    }

    pub fn burst_count(&self) -> usize {
        self.bursts.len()
    }

    pub fn emitter_count(&self) -> usize {
        self.emitters.len()
    }

    pub fn decal_count(&self) -> usize {
        self.decals.len()
    }

    /// Retire every local effect at a whole-world boundary while preserving
    /// this device's particle cap. Entity-anchored emitters carry realm-local
    /// ids, so allowing them across a replacement could attach old smoke to
    /// an unrelated entity that reused the same id.
    pub fn clear(&mut self) {
        self.emitters.clear();
        self.bursts.clear();
        self.lights.clear();
        self.decals.clear();
        self.chains.clear();
        self.live = 0;
    }

    fn next_f32(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 * (1.0 / 16777216.0)
    }

    fn resolve(&mut self, preset: &VfxPresetRef) -> Option<Arc<VfxPreset>> {
        match preset {
            VfxPresetRef::Inline(p) => Some(p.clone()),
            VfxPresetRef::Named(name) => {
                let found = self.defined.get(name).cloned().or_else(|| vfx_preset(name));
                if found.is_none() && !self.unknown_logged.iter().any(|n| n == name) {
                    log!("vfx: unknown preset '{name}'");
                    self.unknown_logged.push(name.clone());
                }
                found
            }
        }
    }

    /// Apply script's requests. Called by the host with whatever it drained.
    pub fn apply(&mut self, requests: &[ParticleRequest]) {
        for r in requests {
            match r {
                ParticleRequest::Emitter { id, anchor, spec } => {
                    let preset = Arc::new(VfxPreset {
                        layers: vec![VfxLayer::from_spec(spec)],
                        looping: true,
                        ..Default::default()
                    });
                    let params = Params { dir: legacy_dir(spec), scale: 1.0, color: None, intensity: 1.0, density: 1.0 };
                    self.start_emitter(*id, *anchor, preset, params);
                }
                ParticleRequest::Burst { at, spec } => {
                    let layer = VfxLayer::from_spec(spec);
                    let params = Params { dir: legacy_dir(spec), scale: 1.0, color: None, intensity: 1.0, density: 1.0 };
                    self.spawn_layer(&layer, *at, Vec3f::default(), Vec3f::default(), layer.count, 0.0, &params);
                }
                ParticleRequest::Stop { id } => self.stop(*id),
                ParticleRequest::Clear => self.clear(),
                ParticleRequest::Vfx(req) => self.spawn_vfx(req),
                ParticleRequest::Define { name, preset } => {
                    self.defined.insert(name.clone(), preset.clone());
                }
                ParticleRequest::Decal(d) => self.add_decal_request(d),
            }
        }
    }

    fn stop(&mut self, id: u64) {
        self.emitters.retain(|e| e.id != id);
        self.lights.retain(|l| l.emitter != id);
    }

    fn start_emitter(&mut self, id: u64, anchor: EmitterAnchor, preset: Arc<VfxPreset>, params: Params) {
        if let Some(e) = self.emitters.iter_mut().find(|e| e.id == id) {
            e.anchor = anchor;
            e.accum.resize(preset.layers.len(), 0.0);
            e.preset = preset;
            e.params = params;
            return;
        }
        let accum = vec![0.0; preset.layers.len()];
        self.emitters.push(Emitter { id, anchor, preset, params, accum, last: None, age: 0.0 });
    }

    /// One `game.vfx` request: a one-shot burst of every layer (plus its
    /// light and decal), or a running emitter for looping presets.
    pub fn spawn_vfx(&mut self, req: &VfxRequest) {
        let Some(preset) = self.resolve(&req.preset) else { return };
        let params = Params {
            dir: if req.dir.length() > 1.0e-6 { req.dir.normalize() } else { vec3f(0.0, 1.0, 0.0) },
            scale: req.scale.max(0.01),
            color: req.color,
            intensity: req.intensity.max(0.0),
            density: req.density.max(0.0),
        };
        if preset.looping {
            let id = if req.id != 0 { req.id } else { 0xF00D_0000_0000 + self.emitters.len() as u64 };
            if let Some(l) = preset.light {
                let at = match req.anchor {
                    EmitterAnchor::Point(p) => p,
                    _ => Vec3f::default(),
                };
                self.lights.retain(|f| f.emitter != id);
                self.lights.push(self.flash_of(&l, at, &params, id));
            }
            // A looping preset's burst-only layers fire once at the start;
            // prewarmed layers start full. A re-sent request (same id,
            // retuning a running effect) does neither again.
            let running = self.emitters.iter().any(|e| e.id == id);
            if let (EmitterAnchor::Point(at), false) = (req.anchor, running) {
                for layer in preset.layers.iter().filter(|l| l.rate <= 0.0) {
                    self.spawn_layer(layer, at, Vec3f::default(), Vec3f::default(), layer.count, 0.0, &params);
                }
                for layer in preset.layers.iter().filter(|l| l.rate > 0.0 && l.prewarm) {
                    self.prewarm_layer(layer, at, &params);
                }
            }
            self.start_emitter(id, req.anchor, preset, params);
            return;
        }
        let EmitterAnchor::Point(at) = req.anchor else {
            // One-shots at an entity are resolved by the host into points;
            // an unresolved anchor has nowhere to be.
            return;
        };
        for layer in &preset.layers {
            self.spawn_layer(layer, at, Vec3f::default(), Vec3f::default(), layer.count, 0.0, &params);
        }
        if let Some(l) = preset.light {
            let flash = self.flash_of(&l, at + params.dir * 0.1, &params, 0);
            self.lights.push(flash);
        }
        if let (Some(d), Some(normal)) = (preset.decal, req.normal) {
            let kind = d.kind;
            let size = d.size * params.scale;
            self.add_decal_request(&DecalRequest {
                kind,
                pos: at,
                normal,
                dir: Vec3f::default(),
                size,
                life: d.life,
                color: d.color,
                chain: 0,
            });
        }
    }

    /// One burst holding a running layer's steady state: `rate × life`
    /// births spread over one lifetime, aged a whole lifetime already, so
    /// every age from newborn to dying is on screen at once.
    fn prewarm_layer(&mut self, layer: &VfxLayer, at: Vec3f, params: &Params) {
        let span = layer.life.1.max(layer.life.0).max(0.0);
        let n = layer.rate * span * 0.5 * (1.0 + layer.life.0 / span.max(1.0e-3));
        let mut warm = layer.clone();
        warm.delay = 0.0;
        warm.duration = span;
        let before = self.bursts.len();
        self.spawn_layer(&warm, at, Vec3f::default(), Vec3f::default(), n / (params.scale.clamp(0.35, 3.0).sqrt()), span, params);
        if let Some(b) = self.bursts.get_mut(before) {
            b.rec.origin.w = span;
        }
    }

    fn flash_of(&self, l: &VfxLight, pos: Vec3f, params: &Params, emitter: u64) -> LightFlash {
        let tint = params.color.map_or(l.color, |c| vec3f(c.x, c.y, c.z));
        LightFlash {
            pos,
            color: tint * (l.intensity * params.intensity),
            radius: l.radius * params.scale.sqrt(),
            life: l.life,
            age: 0.0,
            emitter,
        }
    }

    /// Queue one burst record (split into sheet-sized chunks by the
    /// renderer, not here).
    #[allow(clippy::too_many_arguments)]
    fn spawn_layer(
        &mut self,
        layer: &VfxLayer,
        at: Vec3f,
        path: Vec3f,
        emitter_vel: Vec3f,
        count: f32,
        duration: f32,
        p: &Params,
    ) {
        let n = (count * p.density * p.scale.clamp(0.35, 3.0).sqrt()).round().max(0.0) as usize;
        if n == 0 {
            return;
        }
        let n = n.min(self.cap);
        if n == 0 {
            return;
        }
        self.make_room(n);
        let s = p.scale;
        let (color, color_end) = match p.color {
            Some(c) if layer.tint => {
                // Keep the preset's own birth→death relationship, in the new hue.
                let ratio = |a: f32, b: f32| if a > 1.0e-3 { (b / a).min(2.0) } else { 1.0 };
                (
                    vec4(c.x, c.y, c.z, layer.color.w * c.w),
                    vec4(
                        c.x * ratio(layer.color.x, layer.color_end.x),
                        c.y * ratio(layer.color.y, layer.color_end.y),
                        c.z * ratio(layer.color.z, layer.color_end.z),
                        layer.color_end.w * c.w,
                    ),
                )
            }
            _ => (layer.color, layer.color_end),
        };
        let mut flags = 0.0;
        if layer.collide {
            flags += VFX_COLLIDE;
        }
        if layer.flat {
            flags += VFX_FLAT;
        }
        let inherit = emitter_vel * layer.inherit;
        let seed = self.next_f32() * 997.0;
        let rec = ParticleInstance {
            origin: vec4(at.x, at.y, at.z, 0.0),
            dir: vec4(p.dir.x, p.dir.y, p.dir.z, layer.spread),
            path: vec4(path.x, path.y, path.z, layer.duration.max(duration)),
            inherit: vec4(inherit.x, inherit.y, inherit.z, layer.delay),
            life: vec4(layer.life.0.max(0.0), layer.life.1.max(layer.life.0).max(0.0), layer.speed.0 * s, layer.speed.1 * s),
            size: vec4(layer.size.0 * s, layer.size.1 * s, layer.gravity, layer.drag),
            color,
            color_end,
            look: vec4(layer.intensity * p.intensity, layer.lit, layer.stretch, layer.spin),
            kind: vec4(layer.sprite.id(), if layer.blend == VfxBlend::Add { 1.0 } else { 0.0 }, seed, flags),
            extra: vec4(layer.radius * s, layer.soft * s.max(0.5), -1.0e9, 0.0),
            extra2: vec4(n as f32, layer.fade_in, layer.wobble * s, 0.0),
        };
        self.live += n;
        self.bursts.push(Burst { rec, ground_pending: layer.collide });
    }

    /// Evict the oldest bursts until `incoming` more particles fit the cap.
    fn make_room(&mut self, incoming: usize) {
        if self.live + incoming <= self.cap {
            return;
        }
        let mut drop = 0;
        while drop < self.bursts.len() && self.live + incoming > self.cap {
            let n = self.bursts[drop].rec.count();
            let over = self.live + incoming - self.cap;
            if n > over {
                // Thin this one rather than drop it whole.
                self.bursts[drop].rec.extra2.x = (n - over) as f32;
                self.live -= over;
                break;
            }
            self.live -= n;
            drop += 1;
        }
        self.bursts.drain(..drop);
    }

    fn add_decal_request(&mut self, d: &DecalRequest) {
        let normal = if d.normal.length() > 1.0e-6 { d.normal.normalize() } else { vec3f(0.0, 1.0, 0.0) };
        let (size, life) = d.kind.defaults();
        let size = if d.size > 0.0 { d.size } else { size };
        let life = if d.life > 0.0 { d.life } else { life };
        // Lift off the surface just enough to win the depth test.
        let lift = normal * 0.004;
        if d.chain != 0 {
            let now = self.time;
            let prev = self.chains.iter().position(|c| c.0 == d.chain);
            let mut seg = None;
            if let Some(i) = prev {
                let (_, last, when) = self.chains[i];
                let delta = d.pos - last;
                let len = delta.length();
                if now - when < 0.3 && len < 3.0 {
                    if len < 0.05 {
                        return;
                    }
                    seg = Some((last, delta / len, len));
                }
                self.chains[i] = (d.chain, d.pos, now);
            } else {
                self.chains.push((d.chain, d.pos, now));
            }
            let Some((from, dir, len)) = seg else { return };
            // Flatten the travel into the surface plane.
            let dir = (dir - normal * dir.dot(normal)).normalize();
            let seed = self.next_f32();
            self.decals.push(VfxDecalInstance {
                kind: d.kind,
                pos: from + (d.pos - from) * 0.5 + lift,
                normal,
                dir,
                half_len: len * 0.5,
                half_width: size * 0.5,
                color: d.color,
                seed,
                age: 0.0,
                life,
            });
            return;
        }
        let dir = if d.dir.length() > 1.0e-6 {
            (d.dir - normal * d.dir.dot(normal)).normalize()
        } else {
            // Random roll in the plane.
            let (t1, t2) = basis(normal);
            let a = self.next_f32() * std::f32::consts::TAU;
            t1 * a.cos() + t2 * a.sin()
        };
        let seed = self.next_f32();
        self.decals.push(VfxDecalInstance {
            kind: d.kind,
            pos: d.pos + lift,
            normal,
            dir,
            half_len: size * 0.5,
            half_width: size * 0.5,
            color: d.color,
            seed,
            age: 0.0,
            life,
        });
    }

    /// Step every emitter, burst, light and decal.
    pub fn step(&mut self, dt: f32, entity_pose: &dyn Fn(u64) -> Option<(Vec3f, f32)>) {
        self.step_with_ground(dt, entity_pose, &|_| None)
    }

    /// [`Self::step`] with a ground probe: colliding particles stop on the
    /// ground under their burst's origin (`None` = no ground there).
    /// `entity_pose` resolves entity-anchored emitters to (position, yaw) —
    /// the host passes a lookup into the world it is drawing, so particles
    /// follow objects without the sim carrying particle state, and a
    /// local-offset anchor turns with its body.
    pub fn step_with_ground(
        &mut self,
        dt: f32,
        entity_pose: &dyn Fn(u64) -> Option<(Vec3f, f32)>,
        ground: &dyn Fn(Vec3f) -> Option<f32>,
    ) {
        self.time += dt;
        // Emit.
        let mut pending: Vec<(usize, usize, Vec3f, Vec3f, f32)> = Vec::new();
        for (ei, e) in self.emitters.iter_mut().enumerate() {
            let Some(at) = anchor_pos(e.anchor, entity_pose) else {
                continue;
            };
            e.age += dt;
            let last = e.last.unwrap_or(at);
            e.last = Some(at);
            let vel = if dt > 0.0 { (at - last) / dt } else { Vec3f::default() };
            for (li, layer) in e.preset.layers.iter().enumerate() {
                if layer.rate <= 0.0 || e.age < layer.delay {
                    continue;
                }
                e.accum[li] += layer.rate * dt;
                let n = e.accum[li].floor().max(0.0);
                e.accum[li] -= n;
                if n > 0.0 {
                    pending.push((ei, li, last, vel, n));
                }
            }
        }
        // Entity-anchored emitters whose entity is gone stop emitting; drop
        // them so a long game does not accumulate dead emitters.
        let mut gone = Vec::new();
        for e in &self.emitters {
            if let EmitterAnchor::Entity(id) | EmitterAnchor::EntityLocal(id, _) = e.anchor {
                if entity_pose(id).is_none() {
                    gone.push(e.id);
                }
            }
        }
        for (ei, li, at, vel, n) in pending {
            let preset = self.emitters[ei].preset.clone();
            let mut params = self.emitters[ei].params;
            // Density scales the rate, not the per-frame chunk.
            params.density = 1.0;
            let n = n * self.emitters[ei].params.density;
            let mut layer = preset.layers[li].clone();
            layer.delay = 0.0;
            self.spawn_layer(&layer, at, vel, vel, n, dt, &params);
        }
        for id in gone {
            self.stop(id);
        }
        // Age bursts; resolve grounds; retire.
        for b in self.bursts.iter_mut() {
            b.rec.origin.w += dt;
            if b.ground_pending {
                b.ground_pending = false;
                let o = vec3f(b.rec.origin.x, b.rec.origin.y, b.rec.origin.z);
                if let Some(g) = ground(o) {
                    b.rec.extra.z = g;
                }
            }
        }
        let mut live = self.live;
        self.bursts.retain(|b| {
            let keep = b.rec.origin.w <= b.rec.end();
            if !keep {
                live -= b.rec.count();
            }
            keep
        });
        self.live = live;
        // Lights: one-shots fade; an emitter's light follows it.
        for l in self.lights.iter_mut() {
            l.age += dt;
        }
        let emitters = &self.emitters;
        self.lights.retain(|l| if l.emitter != 0 { emitters.iter().any(|e| e.id == l.emitter) } else { l.age < l.life });
        for l in self.lights.iter_mut().filter(|l| l.emitter != 0) {
            if let Some(e) = emitters.iter().find(|e| e.id == l.emitter) {
                if let Some(p) = e.last {
                    l.pos = p + vec3f(0.0, 0.4, 0.0);
                }
            }
        }
        // Decals age; past the budget, the oldest fade out within a second.
        let over = self.decals.len().saturating_sub(DECAL_SOFT_BUDGET);
        for (i, d) in self.decals.iter_mut().enumerate() {
            d.age += dt;
            if i < over {
                d.life = d.life.min(d.age + 1.0);
            }
        }
        self.decals.retain(|d| d.age < d.life);
        let now = self.time;
        self.chains.retain(|c| now - c.2 < 1.0);
    }

    /// This frame's burst records (GPU path). The renderer splits, sorts and
    /// thins them.
    pub fn instances(&self) -> Vec<ParticleInstance> {
        self.bursts.iter().map(|b| b.rec).collect()
    }

    /// This frame's particles evaluated on the CPU, one explicit record
    /// each (the fallback for backends that should not run the closed form
    /// in a vertex shader).
    pub fn cpu_instances(&self) -> Vec<ParticleInstance> {
        let mut out = Vec::new();
        for b in &self.bursts {
            expand_explicit(&b.rec, &mut out);
        }
        out
    }

    /// Positions of every live particle (tests, debugging).
    pub fn particle_positions(&self) -> Vec<Vec3f> {
        self.cpu_instances().iter().map(|r| vec3f(r.origin.x, r.origin.y, r.origin.z)).collect()
    }

    /// The effects' light flashes for this frame, in lamp units.
    pub fn frame_lights(&self) -> Vec<crate::lightmap::LmLight> {
        self.lights
            .iter()
            .map(|l| {
                let s = if l.emitter != 0 {
                    // A held light flickers like a flame.
                    let t = self.time * 13.0 + l.pos.x;
                    0.85 + 0.1 * t.sin() + 0.05 * (t * 2.3).sin()
                } else {
                    let t = (l.age / l.life.max(1.0e-3)).clamp(0.0, 1.0);
                    (1.0 - t) * (1.0 - t)
                };
                crate::lightmap::LmLight::omni(l.pos, l.color * s, l.radius)
            })
            .collect()
    }

    pub fn decals(&self) -> &[VfxDecalInstance] {
        &self.decals
    }
}

/// Expand one burst record into explicit single-particle records.
pub fn expand_explicit(r: &ParticleInstance, out: &mut Vec<ParticleInstance>) {
    if r.is_explicit() {
        out.push(*r);
        return;
    }
    for i in 0..r.count() {
        if let Some(p) = eval_particle(r, i) {
            let mut e = *r;
            e.origin = vec4(p.pos.x, p.pos.y, p.pos.z, p.age);
            e.dir = vec4(p.vel.x, p.vel.y, p.vel.z, 0.0);
            e.inherit = vec4(0.0, 0.0, 0.0, p.rotation);
            e.life = vec4(p.life, p.life, 0.0, 0.0);
            e.size = vec4(p.size, p.size, 0.0, 0.0);
            e.color = p.color;
            e.color_end = p.color;
            e.kind.z = particle_randoms(i as f32, r.kind.z)[3] * 997.0;
            e.kind.w = VFX_EXPLICIT + if flag(r.kind.w, VFX_FLAT) { VFX_FLAT } else { 0.0 };
            e.path = Vec4f::default();
            e.extra.w = 0.0;
            e.extra2 = vec4(1.0, 0.0, 0.0, 0.0);
            out.push(e);
        }
    }
}

fn anchor_pos(anchor: EmitterAnchor, entity_pose: &dyn Fn(u64) -> Option<(Vec3f, f32)>) -> Option<Vec3f> {
    match anchor {
        EmitterAnchor::Point(p) => Some(p),
        EmitterAnchor::Entity(id) => entity_pose(id).map(|(p, _)| p),
        EmitterAnchor::EntityLocal(id, off) => entity_pose(id).map(|(p, yaw)| {
            // The sim's heading convention: yaw 0 faces -Z, ccw
            // positive. Rotate the local offset into world.
            let (sy, cy) = (yaw.sin(), yaw.cos());
            vec3f(p.x + off.x * cy - off.z * sy, p.y + off.y, p.z + off.x * sy + off.z * cy)
        }),
    }
}

/// The old spec launched up (or down, for a negative speed) in a cone.
fn legacy_dir(spec: &ParticleSpec) -> Vec3f {
    if spec.speed < 0.0 {
        vec3f(0.0, -1.0, 0.0)
    } else {
        vec3f(0.0, 1.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_entities(_: u64) -> Option<(Vec3f, f32)> {
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
        ps.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 1.0, 0.0), spec: s }]);
        assert_eq!(ps.live_count(), 12);
        assert_eq!(ps.burst_count(), 1, "one GPU record per burst, not per particle");
        ps.step(1.0 / 60.0, &no_entities);
        assert_eq!(ps.particle_positions().len(), 12);
    }

    #[test]
    fn emitter_rate_accumulates_across_frames() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Smoke);
        s.rate = 10.0;
        ps.apply(&[ParticleRequest::Emitter { id: 1, anchor: EmitterAnchor::Point(vec3f(0.0, 0.0, 0.0)), spec: s }]);
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
    fn the_cap_evicts_the_oldest_and_lowering_it_trims() {
        let mut ps = ParticleSystem::new(32);
        let mut s = spec(ParticleKind::Spark);
        s.rate = 500.0;
        ps.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 0.0, 0.0), spec: s }]);
        assert_eq!(ps.live_count(), 32, "burst must not exceed the cap");
        ps.set_cap(8);
        assert_eq!(ps.live_count(), 8);
        // A new effect is never the one that goes missing: the old burst
        // makes room.
        s.rate = 6.0;
        ps.apply(&[ParticleRequest::Burst { at: vec3f(5.0, 0.0, 0.0), spec: s }]);
        assert_eq!(ps.live_count(), 8);
        let newest = ps.instances().last().copied().unwrap();
        assert_eq!(newest.count(), 6);
        assert_eq!(newest.origin.x, 5.0);
    }

    #[test]
    fn particles_expire() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Spark);
        s.rate = 5.0;
        s.life = 0.25;
        ps.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 5.0, 0.0), spec: s }]);
        assert_eq!(ps.live_count(), 5);
        for _ in 0..60 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        assert_eq!(ps.live_count(), 0, "all should have aged out");
        assert_eq!(ps.burst_count(), 0);
    }

    #[test]
    fn entity_anchored_emitters_follow_and_then_retire() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Dust);
        s.rate = 60.0;
        ps.apply(&[ParticleRequest::Emitter { id: 7, anchor: EmitterAnchor::Entity(42), spec: s }]);
        let at_x10 = |id: u64| (id == 42).then_some((vec3f(10.0, 0.0, 0.0), 0.0));
        ps.step(1.0 / 60.0, &at_x10);
        assert_eq!(ps.live_count(), 1);
        ps.step(1.0 / 60.0, &at_x10);
        assert!((ps.particle_positions()[0].x - 10.0).abs() < 0.5);
        // Entity gone: the emitter retires rather than lingering forever.
        ps.step(1.0 / 60.0, &no_entities);
        assert_eq!(ps.emitter_count(), 0);
    }

    #[test]
    fn stop_and_clear() {
        let mut ps = ParticleSystem::default();
        let s = spec(ParticleKind::Smoke);
        ps.apply(&[ParticleRequest::Emitter { id: 3, anchor: EmitterAnchor::Point(vec3f(0.0, 0.0, 0.0)), spec: s }]);
        assert_eq!(ps.emitter_count(), 1);
        ps.apply(&[ParticleRequest::Stop { id: 3 }]);
        assert_eq!(ps.emitter_count(), 0);
        let mut burst = s;
        burst.rate = 4.0;
        ps.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 0.0, 0.0), spec: burst }]);
        assert_eq!(ps.live_count(), 4);
        ps.clear();
        assert_eq!(ps.live_count(), 0);
        assert_eq!(ps.emitter_count(), 0);
        assert_eq!(ps.cap(), DEFAULT_PARTICLE_CAP, "realm clear preserves device budget");
    }

    #[test]
    fn particles_fade_and_shrink_over_the_lifetime() {
        let mut ps = ParticleSystem::default();
        let mut s = spec(ParticleKind::Spark);
        s.rate = 1.0;
        s.life = 1.0;
        s.color = vec4f(1.0, 1.0, 1.0, 1.0);
        ps.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 10.0, 0.0), spec: s }]);
        ps.step(1.0 / 60.0, &no_entities);
        let a0 = ps.cpu_instances()[0];
        for _ in 0..20 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        let a1 = ps.cpu_instances()[0];
        assert!(a1.color.w < a0.color.w, "alpha should decay: {} -> {}", a0.color.w, a1.color.w);
        assert!(a1.size.x < a0.size.x, "sparks shrink");
    }

    #[test]
    fn smoke_rises_and_sparks_fall() {
        let mut ps = ParticleSystem::default();
        let mut smoke = spec(ParticleKind::Smoke);
        smoke.rate = 1.0;
        smoke.spread = 0.0;
        ps.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 0.0, 0.0), spec: smoke }]);
        for _ in 0..30 {
            ps.step(1.0 / 60.0, &no_entities);
        }
        assert!(ps.particle_positions()[0].y > 0.0, "smoke should rise");

        let mut ps2 = ParticleSystem::default();
        let mut sp = spec(ParticleKind::Spark);
        sp.rate = 1.0;
        sp.speed = 0.0;
        sp.spread = 0.0;
        ps2.apply(&[ParticleRequest::Burst { at: vec3f(0.0, 5.0, 0.0), spec: sp }]);
        for _ in 0..20 {
            ps2.step(1.0 / 60.0, &no_entities);
        }
        assert!(ps2.particle_positions()[0].y < 5.0, "sparks should fall");
    }

    #[test]
    fn colliding_particles_stop_on_the_probed_ground() {
        let mut ps = ParticleSystem::default();
        let mut req = VfxRequest::named("explosion", vec3f(0.0, 1.0, 0.0));
        req.normal = Some(vec3f(0.0, 1.0, 0.0));
        ps.apply(&[ParticleRequest::Vfx(req)]);
        let floor = |_: Vec3f| Some(0.0);
        for _ in 0..90 {
            ps.step_with_ground(1.0 / 60.0, &no_entities, &floor);
        }
        let debris: Vec<_> = ps
            .instances()
            .into_iter()
            .filter(|r| r.kind.x == VfxSprite::Debris.id())
            .collect();
        assert!(!debris.is_empty());
        for r in &debris {
            for i in 0..r.count() {
                if let Some(p) = eval_particle(r, i) {
                    assert!(p.pos.y >= 0.0, "debris fell through the floor: {}", p.pos.y);
                }
            }
        }
        assert_eq!(ps.decal_count(), 1, "the blast scorched the ground");
    }

    #[test]
    fn presets_light_the_scene_and_the_flash_fades() {
        let mut ps = ParticleSystem::default();
        let mut req = VfxRequest::named("muzzle_flash", vec3f(0.0, 1.5, 0.0));
        req.dir = vec3f(0.0, 0.0, -1.0);
        ps.apply(&[ParticleRequest::Vfx(req)]);
        let l0 = ps.frame_lights();
        assert_eq!(l0.len(), 1);
        ps.step(0.03, &no_entities);
        let l1 = ps.frame_lights();
        assert!(l1[0].color.x < l0[0].color.x);
        ps.step(0.1, &no_entities);
        assert!(ps.frame_lights().is_empty(), "a muzzle flash is gone in a tenth of a second");
    }

    #[test]
    fn looping_presets_run_until_stopped_and_follow_their_entity() {
        let mut ps = ParticleSystem::default();
        let mut req = VfxRequest::named("tyre_smoke", Vec3f::default());
        req.anchor = EmitterAnchor::Entity(9);
        req.id = 77;
        ps.apply(&[ParticleRequest::Vfx(req)]);
        let mut x = 0.0;
        for _ in 0..60 {
            x += 0.5;
            let car = move |id: u64| (id == 9).then_some((vec3f(x, 0.0, 0.0), 0.0));
            ps.step(1.0 / 60.0, &car);
        }
        assert!(ps.live_count() > 20, "34/s for a second");
        // Births are spread along the car's path, not piled at frame starts.
        let xs: Vec<f32> = ps.particle_positions().iter().map(|p| p.x).collect();
        assert!(xs.iter().cloned().fold(f32::MAX, f32::min) < 10.0);
        ps.apply(&[ParticleRequest::Stop { id: 77 }]);
        assert_eq!(ps.emitter_count(), 0);
    }

    #[test]
    fn prewarmed_clouds_are_there_from_the_first_frame() {
        let mut ps = ParticleSystem::default();
        let mut req = VfxRequest::named("cloud", vec3f(0.0, 1500.0, 0.0));
        req.id = 5;
        ps.apply(&[ParticleRequest::Vfx(req.clone())]);
        ps.step(1.0 / 60.0, &no_entities);
        let n0 = ps.particle_positions().len();
        assert!(n0 >= 15, "a full cloud on frame one, got {n0}");
        // Re-sending the request (retuning) does not stack another bank.
        ps.apply(&[ParticleRequest::Vfx(req)]);
        assert_eq!(ps.burst_count(), 1);
        // Two minutes later it is still a cloud (the emitter replaced it).
        for _ in 0..(120 * 4) {
            ps.step(0.25, &no_entities);
        }
        let n1 = ps.particle_positions().len();
        assert!(n1 >= 15 && n1 <= 60, "steady state {n1}");
    }

    #[test]
    fn skid_chains_draw_segments_between_points() {
        let mut ps = ParticleSystem::default();
        let skid = |x: f32| {
            ParticleRequest::Decal(DecalRequest {
                kind: VfxDecalKind::Skid,
                pos: vec3f(x, 0.0, 0.0),
                normal: vec3f(0.0, 1.0, 0.0),
                dir: Vec3f::default(),
                size: 0.25,
                life: 0.0,
                color: vec4(1.0, 1.0, 1.0, 1.0),
                chain: 5,
            })
        };
        ps.apply(&[skid(0.0)]);
        assert_eq!(ps.decal_count(), 0, "the first point only starts the chain");
        ps.step(1.0 / 60.0, &no_entities);
        ps.apply(&[skid(0.5)]);
        ps.step(1.0 / 60.0, &no_entities);
        ps.apply(&[skid(1.0)]);
        assert_eq!(ps.decal_count(), 2);
        let d = ps.decals()[1];
        assert!((d.half_len - 0.25).abs() < 1e-4 && (d.pos.x - 0.75).abs() < 1e-4);
        assert!((d.dir.x - 1.0).abs() < 1e-4);
    }

    #[test]
    fn script_defined_presets_win_over_the_library() {
        let mut ps = ParticleSystem::default();
        let mine = VfxPreset { layers: vec![VfxLayer { count: 3.0, ..VfxLayer::default() }], ..Default::default() };
        ps.apply(&[
            ParticleRequest::Define { name: "hit_spark".into(), preset: Arc::new(mine) },
            ParticleRequest::Vfx(VfxRequest::named("hit_spark", Vec3f::default())),
        ]);
        assert_eq!(ps.live_count(), 3);
        ps.apply(&[ParticleRequest::Vfx(VfxRequest::named("no_such_effect", Vec3f::default()))]);
        assert_eq!(ps.live_count(), 3, "an unknown name draws nothing");
    }

    #[test]
    fn cpu_expansion_matches_the_burst() {
        let mut ps = ParticleSystem::default();
        ps.apply(&[ParticleRequest::Vfx(VfxRequest::named("hit_spark", vec3f(0.0, 1.0, 0.0)))]);
        ps.step(0.02, &no_entities);
        let explicit = ps.cpu_instances();
        assert!(explicit.iter().all(|r| r.is_explicit() && r.count() == 1));
        let total: usize = ps.instances().iter().map(|r| r.count()).sum();
        assert!(explicit.len() <= total && explicit.len() >= total / 2);
    }
}
