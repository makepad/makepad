//! DUO — the two-deck TRANSITION engine (`engine: "transition"`).
//!
//! Where every other engine renders a scene, this one composites the TWO
//! program decks: the host binds deck A into `tex0` and deck B into `tex1`
//! (VjFxView `set_input_texture(0/1, …)`), and drives `p3` with the
//! CROSSFADER POSITION 0→1 (the reserved transition lever — NOT the
//! triangle: a wipe reveals B as the fader travels, so at 0 the output IS
//! deck A and at 1 it IS deck B, which is what makes engagement pop-free).
//!
//! The document subclasses `draw.DrawVjFxDuo` and overrides ONE hook:
//!
//! ```text
//! shader: draw.DrawVjFxDuo {
//!     trans: fn(uv: vec2, t: float) -> vec4 {
//!         // classic left-to-right wipe with a soft edge (p0 = SOFT dial)
//!         let soft = 0.005 + self.user.x * 0.2
//!         let m = smoothstep(t - soft, t + soft, uv.x)
//!         return self.deck_b(uv).mix(self.deck_a(uv), m)
//!     }
//! }
//! ```
//!
//! Helpers in scope: `self.deck_a(uv)` / `self.deck_b(uv)` (bgra samples),
//! `self.aspect()` (viewport w/h for circular masks), the standard signal
//! block (`self.time_beat`, `self.user`, palette…). The default hook is a
//! plain dissolve, so `engine: "transition"` with no shader block IS the
//! classic mix. Docs that only shape `input0` (the premix family) keep
//! using `screen`/`tiles`/… — the host picks the feed by the engine
//! (`EffectDoc` → `Engine::Duo` = wants both decks).
//!
//! Geometry: one clip-space fullscreen quad (the raymarch idiom); the pass
//! camera is ignored. The post chain still runs on top, so a transition doc
//! may add bloom/feedback stages like anything else.

use super::engines::EngineUniforms;
use super::mesh::FxMesh;
use makepad_widgets::*;

#[derive(Clone, Default)]
pub struct DuoConfig {}

pub struct DuoEngine {
    pub cfg: DuoConfig,
    pub(crate) built: bool,
}

impl DuoEngine {
    pub fn new(cfg: DuoConfig) -> Self {
        Self { cfg, built: false }
    }

    /// One fullscreen quad in CLIP SPACE (uv (0,0) = top-left).
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let corners = [
            (vec3f(-1.0, -1.0, 0.0), vec2f(0.0, 1.0)),
            (vec3f(1.0, -1.0, 0.0), vec2f(1.0, 1.0)),
            (vec3f(1.0, 1.0, 0.0), vec2f(1.0, 0.0)),
            (vec3f(-1.0, 1.0, 0.0), vec2f(0.0, 0.0)),
        ];
        let mut ids = [0u32; 4];
        for (k, (pos, uv)) in corners.iter().enumerate() {
            ids[k] = mesh.push_vert(
                *pos,
                k as f32,
                vec3f(0.0, 0.0, 1.0),
                0.0,
                *uv,
                0.0,
                0.0,
            );
        }
        mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms {
            shape: vec4(0.0, 0.0, 0.0, 0.0),
            flow: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // The two-deck compositor. Deliberately tiny: sampling helpers + ONE
    // subclass hook; every classic transition is a preset overriding
    // `trans`, never a branch in here (silent shader size budget).
    mod.draw.DrawVjFxDuo = set_type_default() do #(DrawVjFxDuo::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        tex1: texture_2d(float)

        // THE AUDIO PICTURE (effects/audio_tex.rs) — the live show's sound,
        // sampleable. One float texture rewritten every frame from exactly
        // the stream the beat-sync analysis listens to:
        //   y 0 .. spec_rows-1   SPECTROGRAM ring, x = log-spaced bin
        //                        (30 Hz .. 16 kHz), value 0..1 magnitude
        //                        (0 = -72 dBFS, 1 = 0 dBFS), one row per hop
        //   y spec_rows ..       WAVEFORM ring, x = time inside the row,
        //                        value = the SIGNED sample, -1..1 (0 = silence,
        //                        which is also what an unbound slot reads)
        // audio_dim  = (bins, spec_rows, spec_cursor, wave_cursor)
        // audio_meta = (tex_w, tex_h, wave_rows, hop_secs)
        // audio_env  = (bass, mid, high, rms), smoothed 0..1 — no texture
        //              read needed for a plain level.
        // Each `*_cursor` names the NEWEST row of its ring: that is the
        // unwrap key. Use the two helpers, never a raw uv — and remember a
        // silent rig reads 0 everywhere, so give every look an idle floor.
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 320.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // Spectrum magnitude 0..1. `f` 0..1 = log frequency (0 = 30 Hz,
        // 1 = 16 kHz); `age` 0..1 = how far back (0 = now, 1 = the oldest
        // row kept, about 5.5 s). Silence reads 0.
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }

        // Waveform sample -1..1. `t` 0..1 across the stored window
        // (1 = the newest sample, about 1.4 s deep). Silence reads 0.
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            let uv = vec2((col + 0.5) / self.audio_meta.x, y)
            return self.audio_tex.sample_nearest(uv, 0.0).x
        }
        backface_culling: false
        alpha_blend: false
        depth_write: false

        v_uv: varying(vec2f)

        vertex: fn() {
            // Clip-space passthrough: the pass camera is ignored entirely.
            self.v_uv = self.geom.geom_uv
            self.vertex_pos = vec4(
                self.geom.geom_pos.x,
                self.geom.geom_pos.y,
                0.5,
                1.0
            )
            return self.vertex_pos
        }

        deck_a: fn(uv: vec2) -> vec4 {
            let u = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            return self.tex0.sample_as_bgra(u)
        }

        deck_b: fn(uv: vec2) -> vec4 {
            let u = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            return self.tex1.sample_as_bgra(u)
        }

        // Viewport aspect (w/h) — circular masks stay circular with it.
        aspect: fn() -> float {
            return self.rm.x
        }

        // ---- THE SUBCLASS HOOK -------------------------------------------
        // t is the crossfader 0-1 (p3): return deck A at 0, deck B at 1.
        // Default: the plain dissolve.
        trans: fn(uv: vec2, t: float) -> vec4 {
            return self.deck_a(uv).mix(self.deck_b(uv), t)
        }

        pixel: fn() {
            let t = clamp(self.user.w, 0.0, 1.0)
            let c = self.trans(self.v_uv, t)
            return vec4(c.x, c.y, c.z, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout + `rm` = (viewport aspect, 0, 0, 0), written by
/// the view's dispatch arm (the raymarch convention).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxDuo {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0/p1/p2 = the doc's dials (SOFT/FLIP/…), p3 = THE CROSSFADER 0→1.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub shape: Vec4f,
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
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
    #[live(vec4(0.0, 1.0, 0.0, 0.0))]
    pub fog: Vec4f,
    /// (viewport aspect, unused, unused, unused).
    #[live(vec4(1.7777, 0.0, 0.0, 0.0))]
    pub rm: Vec4f,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duo_builds_one_fullscreen_quad_once() {
        let mut e = DuoEngine::new(DuoConfig::default());
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(mesh.verts.len() / super::super::mesh::VERT_FLOATS, 4);
        assert_eq!(mesh.idx.len(), 6);
    }
}
