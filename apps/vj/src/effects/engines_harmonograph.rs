//! Harmonograph Loom — a Victorian harmonograph drawing in the air.
//!
//! ONE ribbon (or a few chorus strands) whose every point is computed IN THE
//! VERTEX SHADER as the damped sum of pendulum sines:
//! `x(t) = Σ Aᵢ·e^(−dᵢt)·sin(wᵢt + pᵢ)` — the mesh is a static strip of
//! curve parameters and the whole figure lives in the uniforms. The figure
//! MORPHS tempo-locked: every `morph_beats` beats a new coefficient set is
//! drawn from a hash of the figure index and the shader eases between the
//! outgoing and incoming figures near the boundary — it never draws the
//! same rosette twice. The standard growth sweep (`grow: "loop"`) draws the
//! figure pen-first each cycle.
//!
//! Pure static-mesh class: zero CPU per frame after the one upload.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   a_id   = strand index
//!   a_aux  = curve parameter t 0..1 (the whole figure)
//!   a_r0   = per-strand rnd (phase/hue offset seed)
//!   a_r1   = ribbon side ±1
//!   uv     = (t, strand 0..1)
//!   normal = per-strand unit rnd vector (decorrelation seed, spare)
//!   geom_pos unused (position is pure shader math)
//!
//! # Document keys (`engine: "harmonograph"`)
//! `segments` (1600), `strands` (1..6), `freq_x`/`freq_y`/`freq_z`
//! (2/3/1 — near-integer ratios are the figure family), `damping` (0.45),
//! `detune` (0.08 — frequency scatter per figure; `p0` binding scales),
//! `turns` (5 pendulum swings across the figure), `width` (0.07; `p1`
//! pumps), `morph_beats` (16). `p2` adds z-depth, `p3` spins the hue.
//! Shader hook: `fx_color(t, attr, normal, wpos)`.

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

pub struct HarmonographConfig {
    pub segments: usize,
    pub strands: usize,
    pub freq_x: f32,
    pub freq_y: f32,
    pub freq_z: f32,
    pub damping: f32,
    /// Relative frequency scatter drawn per figure (0..0.6 useful).
    pub detune: f32,
    /// Pendulum swings across the whole figure (density of the rosette).
    pub turns: f32,
    pub width: f32,
    /// Beats per figure — the morph is tempo-locked by construction.
    pub morph_beats: f32,
    pub seed: u64,
}

impl Default for HarmonographConfig {
    fn default() -> Self {
        Self {
            segments: 1600,
            strands: 1,
            freq_x: 2.0,
            freq_y: 3.0,
            freq_z: 1.0,
            damping: 0.45,
            detune: 0.08,
            turns: 5.0,
            width: 0.07,
            morph_beats: 16.0,
            seed: 24,
        }
    }
}

pub struct HarmonographEngine {
    pub cfg: HarmonographConfig,
    pub(crate) built: bool,
}

impl HarmonographEngine {
    pub fn new(cfg: HarmonographConfig) -> Self {
        Self { cfg, built: false }
    }

    /// Static strip(s): two verts per curve sample, quads between
    /// consecutive samples. Position is entirely shader math off `t`.
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let segments = self.cfg.segments.clamp(64, 6000);
        let strands = self.cfg.strands.clamp(1, 6);
        let mut rng = FxRng::new(self.cfg.seed);
        for s in 0..strands {
            let strand01 = if strands > 1 { s as f32 / (strands - 1) as f32 } else { 0.0 };
            let srnd = rng.next_f32();
            // Spare decorrelation vector on the normal channel.
            let z = rng.range(-1.0, 1.0);
            let a = rng.range(0.0, std::f32::consts::TAU);
            let sl = (1.0 - z * z).max(0.0).sqrt();
            let svec = vec3f(sl * a.cos(), z, sl * a.sin());
            let mut prev: Option<(u32, u32)> = None;
            for k in 0..=segments {
                let t = k as f32 / segments as f32;
                let l = mesh.push_vert(
                    vec3f(0.0, 0.0, 0.0),
                    s as f32,
                    svec,
                    t,
                    vec2f(t, strand01),
                    srnd,
                    -1.0,
                );
                let r = mesh.push_vert(
                    vec3f(0.0, 0.0, 0.0),
                    s as f32,
                    svec,
                    t,
                    vec2f(t, strand01),
                    srnd,
                    1.0,
                );
                if let Some((pl, pr)) = prev {
                    mesh.push_quad(pl, pr, r, l);
                }
                prev = Some((l, r));
            }
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        // Rust's clamp propagates NaN — sanitize first so a hostile doc
        // value can never poison the uniform stream (panic/NaN-free law).
        let san = |v: f32, d: f32| if v.is_finite() { v } else { d };
        EngineUniforms {
            shape: vec4(
                san(self.cfg.turns, 5.0).clamp(0.5, 24.0),
                san(self.cfg.damping, 0.45).clamp(0.0, 3.0),
                san(self.cfg.width, 0.07).clamp(0.002, 1.0),
                san(self.cfg.morph_beats, 16.0).clamp(1.0, 256.0),
            ),
            flow: vec4(
                san(self.cfg.freq_x, 2.0).clamp(0.25, 16.0),
                san(self.cfg.freq_y, 3.0).clamp(0.25, 16.0),
                san(self.cfg.freq_z, 1.0).clamp(0.0, 16.0),
                san(self.cfg.detune, 0.08).clamp(0.0, 0.6),
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Harmonograph Loom: the curve is evaluated twice per vertex (t and
    // t+ε) and the strip expands across the view-projected tangent — the
    // ribbon always faces the camera whatever the figure's 3D bend (the
    // ribbons-shader technique). Figure k's coefficients are hashes of k;
    // near a morph boundary both figures are evaluated and blended.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxHarmono = set_type_default() do #(DrawVjFxHarmono::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)
        backface_culling: false
        alpha_blend: true
        depth_write: false

        v_color: varying(vec4f)
        v_side: varying(float)

        h1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        // The damped pendulum sum for figure k at curve parameter t.
        fig_pos: fn(t: float, k: float, sphase: float) -> vec3 {
            let tau = 6.2831853
            let tt = t * self.shape.x * tau
            let env = exp(0.0 - self.shape.y * t * 3.0)
            let det = clamp(self.flow.w * (1.0 + self.user.x), 0.0, 0.9)
            let wx = self.flow.x * (1.0 + det * (self.h1(k * 7.13 + 1.0) - 0.5))
            let wy = self.flow.y * (1.0 + det * (self.h1(k * 7.13 + 2.0) - 0.5))
            let wz = self.flow.z * (1.0 + det * (self.h1(k * 7.13 + 3.0) - 0.5))
            let px = self.h1(k * 3.31 + 4.0) * tau + sphase
            let py = self.h1(k * 3.31 + 5.0) * tau + sphase * 1.7
            let pz = self.h1(k * 3.31 + 6.0) * tau
            // Second pendulum per axis at a small-integer multiple — keeps
            // the figure family closed (rosettes/shells, never mush).
            let a2 = 0.30 + 0.45 * self.h1(k * 5.91 + 7.0)
            let w2 = 1.0 + floor(self.h1(k * 5.91 + 8.0) * 3.0)
            let x = sin(wx * tt + px) + a2 * sin(wx * w2 * tt + px * 1.7)
            let y = sin(wy * tt + py) + a2 * sin(wy * w2 * tt + py * 0.6)
            let z = sin(wz * tt + pz)
            let zd = clamp(0.55 + self.user.z, 0.0, 3.0)
            return vec3(x, y * 0.85, z * zd) * (env * 2.4 / (1.0 + a2))
        }

        // Tempo-locked figure morph: beat / morph_beats indexes the figure.
        curve_at: fn(t: float, sphase: float) -> vec3 {
            let fb = self.time_beat.y / max(self.shape.w, 0.5)
            let k = floor(fb)
            let u = fract(fb)
            let blend = smoothstep(0.75, 1.0, u)
            let pa = self.fig_pos(t, k, sphase)
            let pb = self.fig_pos(t, k + 1.0, sphase)
            return mix(pa, pb, blend)
        }

        // Document hook: the ink. t = curve parameter, attr.z = strand rnd.
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let strand01 = self.geom.geom_uv.y
            let hue = fract(t * 0.85 + strand01 * 0.34 + self.user.w)
            return self.col_a.mix(self.col_b, hue)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let t = attr.y
            let side = attr.w
            let sphase = attr.z * 6.2831853
            // Growth front: anim.z sweeps 0..1.3 (grow: "loop"), scaled so
            // the figure completes before the cycle restarts; grow: "off"
            // holds the figure fully drawn.
            let front = clamp(self.anim.z * 0.79, 0.0, 1.0)
            let p = self.curve_at(t, sphase)
            let q = self.curve_at(t + 0.002, sphase)
            let world = self.draw_list.view_transform * vec4(p.x, p.y, p.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            let wq = self.draw_list.view_transform * vec4(q.x, q.y, q.z, 1.0)
            let vq = self.draw_pass.camera_view * wq
            let tan_view = vq.xyz - view_pos.xyz
            let across = normalize(vec2(0.0 - tan_view.y, tan_view.x) + vec2(0.0001, 0.0))
            let vis = step(t, front)
            // The pen: white-hot right at the growth front.
            let tip = exp((t - front) * 40.0) * vis
            // Damping envelope: as the figure decays toward the center the
            // curve bunches up — taper width AND brightness with the same
            // envelope, or thousands of additive segments blow out white.
            let envt = exp(0.0 - self.shape.y * t * 3.0)
            let taper = (0.45 + 0.55 * sin(3.14159265 * clamp(t, 0.0, 1.0)))
                * (0.30 + 0.70 * envt)
            let width = self.shape.z * clamp(1.0 + self.user.y, 0.05, 6.0)
                * taper * (1.0 + tip * 1.5)
            let offset = across * (side * width)
            let billboard = vec4(
                view_pos.x + offset.x,
                view_pos.y + offset.y,
                view_pos.z,
                view_pos.w
            )
            let ink = self.fx_color(t, attr, self.geom.geom_normal, world.xyz)
            let lit = ink.mix(self.col_c, tip)
            let bright = (0.30 + 0.85 * tip + 0.25 * self.time_beat.w)
                * (0.25 + 0.75 * envt) * self.fog.y
            // Wet ink: freshly drawn passages glow, older ones dry down.
            // Ghost floor 0.045: the undrawn figure stays faintly present so
            // a growth loop never shows an empty frame (VJ dead-air law).
            let alpha = max(vis * (0.30 + 0.70 * exp((t - front) * 3.0)), 0.045)
            self.v_color = vec4(lit.xyz * bright, alpha)
            self.v_side = side
            self.vertex_pos = self.draw_pass.camera_projection * billboard
            return self.vertex_pos
        }

        pixel: fn() {
            let edge = 1.0 - abs(self.v_side)
            let a = smoothstep(0.0, 0.6, edge + 0.55) * self.v_color.w
            return vec4(self.v_color.xyz * a, 0.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxHarmono {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = detune scale, p1 = width pump, p2 = z-depth add, p3 = hue spin.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (turns, damping, width, morph_beats).
    #[live(vec4(5.0, 0.45, 0.07, 16.0))]
    pub shape: Vec4f,
    /// (freq_x, freq_y, freq_z, detune).
    #[live(vec4(2.0, 3.0, 1.0, 0.08))]
    pub flow: Vec4f,
    #[live(vec4(0.28, 0.94, 1.0, 1.0))]
    pub col_a: Vec4f,
    #[live(vec4(1.0, 0.25, 0.63, 1.0))]
    pub col_b: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub col_c: Vec4f,
    #[live(vec4(0.01, 0.012, 0.03, 1.0))]
    pub col_bg: Vec4f,
    /// (fog density, emissive gain, tex mix, unused).
    #[live(vec4(0.05, 1.0, 0.0, 0.0))]
    pub fog: Vec4f,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonograph_strip_topology() {
        let mut e = HarmonographEngine::new(HarmonographConfig {
            segments: 100,
            strands: 3,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(mesh.vertex_count(), 3 * 101 * 2);
        assert_eq!(mesh.triangle_count(), 3 * 100 * 2);
        // t must be monotone within each strand and sides must alternate.
        let floats = super::super::mesh::VERT_FLOATS;
        for pair in mesh.verts.chunks(floats * 2) {
            assert_eq!(pair[7], pair[floats + 7], "L/R share the same t");
            assert_eq!(pair[11], -1.0);
            assert_eq!(pair[floats + 11], 1.0);
        }
    }

    #[test]
    fn harmonograph_degenerate_params_stay_safe() {
        let mut e = HarmonographEngine::new(HarmonographConfig {
            segments: 0,
            strands: 0,
            freq_x: -8.0,
            damping: f32::NAN,
            morph_beats: 0.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() >= 65 * 2, "segment floor holds");
        let u = e.uniforms();
        assert!(u.flow.x >= 0.25, "frequency clamps to a positive floor");
        assert!(u.shape.w >= 1.0, "morph_beats floor prevents div blowup");
        // NaN damping must not escape into the uniform stream.
        assert!(u.shape.y.is_finite(), "NaN doc value must be sanitized");
    }
}
