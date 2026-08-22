//! The sim-field engines: STATEFUL GPU particles + the stable-fluids dye
//! renderer. Both consume SIM FIELDS (sim.rs) — the float ping-pong
//! render-target primitive — instead of deriving motion in closed form.
//!
//! # `engine: "simswarm"` — particles with real state
//! Particle state (pos/vel/age) lives in a float texture (texel =
//! particle); an update pass integrates forces (curl noise, orbiting
//! attractor + tangential swirl, gravity, beat impulses) every frame, and
//! the draw pass's VERTEX shader fetches state by instance-id → texel to
//! place velocity-stretched billboard quads. The CPU uploads the quad
//! sheet ONCE and never touches a particle again.
//!
//! Document keys: `count` (9216 → state side 96, ≤25600), `size` (0.10),
//! `stretch` (0.035 velocity elongation), `speed_color` (0.12 speed→hue
//! gain) + the state-field force keys (`curl`, `attract`, `swirl`,
//! `gravity`, `impulse`, `drag`, `max_speed`, `life`, `bound`, `spawn`) —
//! all animatable, read by the auto-created particles field named "state"
//! (declare your own `fields:` entry named "state" to override wholesale).
//! Hooks: `fx_color`, `fx_sprite` (draw), and the FIELD's `update:`
//! subclass owns the motion (`force: fn(p, v, id) -> vec3`).
//!
//! # `engine: "fluid"` — stable-fluids dye, rendered gorgeous
//! Auto-creates a fluid field named "dye" (`grid` → res, `iters` budget).
//! The scene pass is the field's view shader: dye through the palette +
//! glow, velocity shimmer, optional input0 underneath warped by the flow
//! (`p0` = warp, `p1` = input mix 0..1). Splats inject on the beat. The
//! dye is also `input0: "field:dye"`-consumable from ANY other effect.

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

pub struct SwarmConfig {
    /// Requested particle count; rounded up to a square state side.
    pub count: usize,
    pub size: f32,
    /// Velocity → sprite elongation gain.
    pub stretch: f32,
    /// Speed → color ramp gain (flow.y in the draw shader).
    pub speed_color: f32,
    /// Camera framing radius (mirrors the field's `bound`).
    pub bound: f32,
    /// Mirrors the field's `life` for the age fade in the draw shader.
    pub life: f32,
    /// The particles field this engine draws from.
    pub state_field: String,
    pub seed: u64,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            count: 9216,
            size: 0.10,
            stretch: 0.035,
            speed_color: 0.12,
            bound: 6.0,
            life: 7.0,
            state_field: "state".to_string(),
            seed: 3,
        }
    }
}

pub struct SwarmEngine {
    pub cfg: SwarmConfig,
    /// State-texture side S (texture is 2S x S).
    pub side: usize,
    pub built: bool,
}

impl SwarmEngine {
    pub fn new(mut cfg: SwarmConfig) -> Self {
        let side = (cfg.count.clamp(256, 25_600) as f32).sqrt().ceil() as usize;
        let side = side.clamp(16, 160);
        cfg.count = side * side;
        Self { cfg, side, built: false }
    }

    /// One billboard quad per particle; the vertex shader replaces the
    /// center with the fetched state texel. Corner in geom_pos, corner01
    /// in uv, particle id in a_id — the particles-sheet conventions.
    pub fn build(&mut self, mesh: &mut FxMesh) {
        let mut rng = FxRng::new(self.cfg.seed);
        for i in 0..self.cfg.count {
            let id = i as f32;
            // Direction seed kept for hook authors (unused by the base VS).
            let dir = vec3f(
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
            );
            let r0 = rng.next_f32();
            let r1 = rng.next_f32();
            let c = [
                (vec3f(-0.5, -0.5, 0.0), vec2f(0.0, 0.0)),
                (vec3f(0.5, -0.5, 0.0), vec2f(1.0, 0.0)),
                (vec3f(0.5, 0.5, 0.0), vec2f(1.0, 1.0)),
                (vec3f(-0.5, 0.5, 0.0), vec2f(0.0, 1.0)),
            ];
            let mut idx = [0u32; 4];
            for (k, (pos, uv)) in c.iter().enumerate() {
                idx[k] = mesh.push_vert(*pos, id, dir, 0.0, *uv, r0, r1);
            }
            mesh.push_quad(idx[0], idx[1], idx[2], idx[3]);
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms {
            shape: vec4(
                self.side as f32,
                self.cfg.size.clamp(0.005, 2.0),
                self.cfg.stretch.clamp(0.0, 2.0),
                self.cfg.life.clamp(0.5, 60.0),
            ),
            flow: vec4(
                self.cfg.bound.clamp(0.5, 100.0),
                self.cfg.speed_color.clamp(0.0, 4.0),
                0.0,
                0.0,
            ),
        }
    }
}

pub struct FluidConfig {
    /// Sim grid resolution (the auto-created field's res).
    pub grid: usize,
    /// The fluid field this engine renders.
    pub field: String,
}

impl Default for FluidConfig {
    fn default() -> Self {
        Self { grid: 144, field: "dye".to_string() }
    }
}

/// No mesh at all — the scene pass is the fluid field's view shader.
pub struct FluidEngine {
    pub cfg: FluidConfig,
}

impl FluidEngine {
    pub fn new(cfg: FluidConfig) -> Self {
        Self { cfg }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swarm_rounds_count_to_a_square_side() {
        let e = SwarmEngine::new(SwarmConfig { count: 5000, ..Default::default() });
        assert_eq!(e.side * e.side, e.cfg.count);
        assert!(e.side >= 16 && e.side <= 160);
        let tiny = SwarmEngine::new(SwarmConfig { count: 1, ..Default::default() });
        assert!(tiny.cfg.count >= 256);
    }

    #[test]
    fn swarm_sheet_is_one_quad_per_particle() {
        let mut e = SwarmEngine::new(SwarmConfig { count: 1024, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(mesh.vertex_count(), e.cfg.count * 4);
        assert_eq!(mesh.triangle_count(), e.cfg.count * 2);
    }
}
