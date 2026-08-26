//! Copper Reef — rasterbars reborn as 3D lacquered slabs: a stack of
//! full-width horizontal bars that sine-dance, pile up on the kick, scissor
//! through each other and rotate into a curtain — the whole choreography a
//! pure function of (beat, phase, bar index) in the vertex shader.
//!
//! STATIC mesh: N real boxes with per-face normals, built once. Each frame
//! the VS evaluates a choreography function selected by a mode uniform for
//! every bar — and evaluates it TWICE (`mode`/`mode_b`), crossfading by the
//! `p3` binding, so a document can morph sine⇄scissor on the bar phase with
//! zero new code. Thickness is applied in the shader (`p1` pumps it on the
//! beat — the classic rasterbar breath).
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   geom_pos = (x world along the bar, y local ±0.5 UNIT — the shader
//!              scales it by live thickness, z world ±depth/2)
//!   a_id     = bar index
//!   normal   = canonical face normal (shader re-tilts it with the bar)
//!   a_aux    = local y within the bar 0..1 (the copper-gradient axis)
//!   uv       = (x along bar 0..1, face code /5)
//!   a_r0     = per-bar choreography hash
//!   a_r1     = bar01 (index / (bars−1))
//!
//! # Document keys (`engine: "copperbars"`)
//! `bars` (24, 4..64), `mode` + `mode_b` ("sine" | "pile" | "scissor" |
//! "curtain"; mode_b defaults to mode), `width` (15 — full bar length),
//! `span` (6.5 vertical travel), `thickness` (0.42), `depth` (1.2),
//! `amplitude` (1.6), `weave` (1.2 — z interleave depth), `metal` (3.0
//! gradient hardness), `drop` (7.0 — pile-mode drop height). Bindings:
//! `p0` = amplitude gain (`"0.3 + 0.9*bass + 0.5*pulse"`), `p1` = thickness
//! pump (`"0.4 + 0.9*env(phase)"`), `p2` free, `p3` = mode crossfade 0..1
//! (`"0.5 - 0.5*cos(bar*tau)"`). Hook: `fx_color(t = gradient axis 0..1,
//! attr = (id, x01, seed, bar01), normal, wpos)` — the bar material fn.
//!
//! Content coupling (`content:` → `fog.z`): the bars catch the video as
//! environment light (mirror-direction env map shaped by the copper band).

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

#[derive(Clone, Copy, PartialEq)]
pub enum CopperMode {
    Sine = 0,
    Pile = 1,
    Scissor = 2,
    Curtain = 3,
}

impl CopperMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sine" | "stack" => Self::Sine,
            "pile" | "bounce" => Self::Pile,
            "scissor" | "interleave" => Self::Scissor,
            "curtain" | "blinds" => Self::Curtain,
            _ => return None,
        })
    }
}

pub struct CopperConfig {
    pub bars: usize,
    /// Full bar length along X.
    pub width: f32,
    /// Vertical travel span.
    pub span: f32,
    pub thickness: f32,
    /// Bar depth along Z.
    pub depth: f32,
    /// Choreography amplitude.
    pub amplitude: f32,
    /// Z interleave depth.
    pub weave: f32,
    /// Gradient hardness (higher = harder classic rasterbar bands).
    pub metal: f32,
    pub mode: CopperMode,
    /// Crossfade target (p3 blends towards it). Defaults to `mode`.
    pub mode_b: CopperMode,
    /// Pile-mode drop height.
    pub drop: f32,
    /// Pulses per beat (mirrors the document's beat_rate).
    pub rate: f32,
    pub seed: u64,
}

impl Default for CopperConfig {
    fn default() -> Self {
        Self {
            bars: 24,
            width: 15.0,
            span: 6.5,
            thickness: 0.42,
            depth: 1.2,
            amplitude: 1.6,
            weave: 1.2,
            metal: 3.0,
            mode: CopperMode::Sine,
            mode_b: CopperMode::Sine,
            drop: 7.0,
            rate: 1.0,
            seed: 7,
        }
    }
}

pub struct CopperEngine {
    pub cfg: CopperConfig,
    pub(crate) built: bool,
    pub placed: usize,
}

impl CopperEngine {
    pub fn new(cfg: CopperConfig) -> Self {
        Self { cfg, built: false, placed: 0 }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    /// Static build: one 6-face box per bar. Y is UNIT (±0.5) so the shader
    /// owns live thickness; X and Z are world-sized at build.
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let bars = self.cfg.bars.clamp(4, 64);
        let w2 = Self::san(self.cfg.width, 15.0).clamp(2.0, 80.0) * 0.5;
        let d2 = Self::san(self.cfg.depth, 1.2).clamp(0.05, 8.0) * 0.5;
        let mut rng = FxRng::new(self.cfg.seed);
        // Local unit box: x ±w2 (world), y ±0.5 (unit), z ±d2 (world).
        // Face table: (normal, corners, face code).
        let faces: [([f32; 3], [[f32; 3]; 4], f32); 6] = [
            ([0.0, 0.0, 1.0],
             [[-w2, -0.5, d2], [w2, -0.5, d2], [w2, 0.5, d2], [-w2, 0.5, d2]], 0.0),
            ([0.0, 0.0, -1.0],
             [[w2, -0.5, -d2], [-w2, -0.5, -d2], [-w2, 0.5, -d2], [w2, 0.5, -d2]], 1.0),
            ([0.0, 1.0, 0.0],
             [[-w2, 0.5, d2], [w2, 0.5, d2], [w2, 0.5, -d2], [-w2, 0.5, -d2]], 2.0),
            ([0.0, -1.0, 0.0],
             [[-w2, -0.5, -d2], [w2, -0.5, -d2], [w2, -0.5, d2], [-w2, -0.5, d2]], 3.0),
            ([1.0, 0.0, 0.0],
             [[w2, -0.5, d2], [w2, -0.5, -d2], [w2, 0.5, -d2], [w2, 0.5, d2]], 4.0),
            ([-1.0, 0.0, 0.0],
             [[-w2, -0.5, -d2], [-w2, -0.5, d2], [-w2, 0.5, d2], [-w2, 0.5, -d2]], 5.0),
        ];
        for bar in 0..bars {
            let seed = rng.next_f32();
            let bar01 = if bars > 1 { bar as f32 / (bars - 1) as f32 } else { 0.5 };
            for (n, corners, face) in &faces {
                let normal = vec3f(n[0], n[1], n[2]);
                let mut ids = [0u32; 4];
                for (k, c) in corners.iter().enumerate() {
                    let x01 = (c[0] / (2.0 * w2) + 0.5).clamp(0.0, 1.0);
                    ids[k] = mesh.push_vert(
                        vec3f(c[0], c[1], c[2]),
                        bar as f32,
                        normal,
                        c[1] + 0.5,
                        vec2f(x01, face / 5.0),
                        seed,
                        bar01,
                    );
                }
                mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
            }
        }
        self.placed = bars;
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        let mode_pack = self.cfg.mode as i32 as f32 + self.cfg.mode_b as i32 as f32 * 8.0;
        EngineUniforms {
            shape: vec4(
                mode_pack,
                san(self.cfg.thickness, 0.42).clamp(0.02, 4.0),
                san(self.cfg.span, 6.5).clamp(0.5, 40.0),
                san(self.cfg.rate, 1.0).clamp(0.05, 8.0),
            ),
            flow: vec4(
                san(self.cfg.amplitude, 1.6).clamp(0.0, 20.0),
                san(self.cfg.weave, 1.2).clamp(0.0, 10.0),
                san(self.cfg.metal, 3.0).clamp(0.5, 24.0),
                san(self.cfg.drop, 7.0).clamp(0.0, 40.0),
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Copper Reef: solid depth-tested slabs. The choreography fn returns
    // (y offset, z offset, tilt about the long axis, landing flash) per
    // mode; the vertex evaluates it for mode A and B and crossfades by p3.
    // The copper gradient itself is PER PIXEL (crisp bands + specular line)
    // through the overridable fx_color hook.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxCopper = set_type_default() do #(DrawVjFxCopper::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)

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
        depth_write: true

        // (id, x01, seed, bar01) — the hook's raw material.
        v_attr: varying(vec4f)
        // (gradient axis 0..1, landing flash, unused, unused)
        v_misc: varying(vec4f)
        v_normal: varying(vec3f)
        v_world: varying(vec3f)

        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        // One bar's placement: returns (y, z, tilt, flash). `xb` is the
        // continuous pulse position beat*rate — every mode locks to it.
        choreo: fn(mode: float, idv: float, bar01: float, seedv: float,
                   amp: float, span: float, xb: float) -> vec4 {
            let home = (bar01 - 0.5) * span
            if mode < 0.5 {
                // SINE STACK: the classic — phase-spread sine dance that
                // breathes on the pulse, weaving in depth.
                let ph = self.time_beat.x * 1.25 + bar01 * 5.2 + seedv * 3.1
                let y = home + sin(ph) * amp * (0.8 + 0.4 * self.time_beat.w)
                let z = cos(self.time_beat.x * 0.9 + bar01 * 8.8) * self.flow.y
                let tilt = sin(ph * 0.5) * 0.06
                return vec4(y, z, tilt, 0.0)
            }
            if mode < 1.5 {
                // BOUNCE PILE: every pulse the stack drops from above and
                // lands staggered (per-bar per-hit hash) — bars land ON the
                // kick and flash white at touchdown.
                let hb = self.hash1(seedv * 61.0 + floor(xb) * 7.13)
                let speedf = 1.25 + hb * 1.1
                let tau1 = clamp(self.time_beat.z * speedf, 0.0, 1.0)
                let stack = 0.0 - span * 0.5 + bar01 * span * 0.62
                let y = stack + (1.0 - tau1) * (1.0 - tau1) * self.flow.w
                let z = (seedv - 0.5) * self.flow.y * 0.8
                let tilt = (1.0 - tau1) * (seedv - 0.5) * 0.5
                let land = 1.0 / speedf
                let flash = pow(
                    clamp(1.0 - abs(self.time_beat.z - land) * 8.0, 0.0, 1.0),
                    2.0
                )
                return vec4(y, z, tilt, flash)
            }
            if mode < 2.5 {
                // SCISSOR: bars HOLD their mirrored posts and SNAP-cross at
                // mid-pulse (the crossing IS the accent — a slow cosine
                // would leave the stack pinched half the time). Continuous
                // across pulses: each pulse un-mirrors the previous one.
                let par = modf(idv, 2.0) * 2.0 - 1.0
                let k = floor(xb)
                let f = xb - k
                let base = modf(k, 2.0)
                let cross01 = smoothstep(0.35, 0.65, f)
                let c = base + (1.0 - 2.0 * base) * cross01
                // Holds breathe a little (never a dead-still frame); the
                // crossing still owns the motion.
                let sway = sin(self.time_beat.x * 1.7 + bar01 * 7.0) * amp * 0.12
                let y = home * (1.0 - 2.0 * c) + sway
                let z = par * self.flow.y * sin(cross01 * 3.14159265)
                let tilt = par * 0.10 * sin(cross01 * 6.2831853)
                let flash = pow(clamp(1.0 - abs(f - 0.5) * 5.0, 0.0, 1.0), 2.0) * 0.6
                return vec4(y, z, tilt, flash)
            }
            // CURTAIN: bars roll towards vertical in a wave that travels
            // down the stack, locked to the pulse clock.
            let c2 = 0.5 - 0.5 * cos(xb * 1.5707963 + bar01 * 2.2 + seedv * 0.4)
            let y = home * (1.0 - 0.25 * c2)
            let z = sin(c2 * 3.14159265) * self.flow.y * 0.4
            let tilt = c2 * 1.45 + self.time_beat.w * 0.12
            return vec4(y, z, tilt, 0.0)
        }

        // Document hook: the bar material. t = gradient axis (local y 0..1),
        // attr = (id, x01, seed, bar01). Default: hard two-stop copper
        // gradient + a specular line that brightens on the pulse.
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let band = pow(clamp(sin(t * 3.14159265), 0.0, 1.0), max(self.flow.z, 0.5))
            let base = self.col_b.mix(self.col_a, band)
            let sline = 1.0 - clamp(abs(t - 0.68) * 9.0, 0.0, 1.0)
            let spec = pow(sline, 3.0) * (0.5 + self.time_beat.w * 0.8)
            let vary = 0.85 + 0.30 * attr.z
            return vec4(base.xyz * vary + self.col_c.xyz * spec, 1.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            // Unpack modes; xb = continuous pulse position (beat * rate).
            let pack = self.shape.x
            let mb = floor(pack / 8.0)
            let ma = pack - mb * 8.0
            let xb = self.time_beat.y * max(self.shape.w, 0.05)
            let amp = self.flow.x
                * clamp(0.55 + self.user.x + self.time_beat.w * 0.30, 0.10, 4.0)
            let ca = self.choreo(ma, attr.x, attr.w, attr.z, amp, self.shape.z, xb)
            let cb = self.choreo(mb, attr.x, attr.w, attr.z, amp, self.shape.z, xb)
            let ch = ca.mix(cb, clamp(self.user.w, 0.0, 1.0))
            // Live thickness: p1 pumps it, the pulse keeps a floor breath.
            let th = self.shape.y
                * clamp(0.55 + 0.45 * max(self.user.y, self.time_beat.w), 0.15, 3.0)
            let ly = self.geom.geom_pos.y * th
            let lz = self.geom.geom_pos.z
            let cs = cos(ch.z)
            let sn = sin(ch.z)
            let pos = vec3(
                self.geom.geom_pos.x,
                ly * cs - lz * sn + ch.x,
                ly * sn + lz * cs + ch.y
            )
            let n0 = self.geom.geom_normal
            let n = vec3(n0.x, n0.y * cs - n0.z * sn, n0.y * sn + n0.z * cs)
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_normal = n
            self.v_attr = vec4(attr.x, self.geom.geom_uv.x, attr.z, attr.w)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            // v_misc.zw = SCREEN uv — the address the content coupling
            // mirrors the channel video from (the slab stack becomes a
            // venetian blind over the picture).
            let ndc = self.vertex_pos.xy / max(self.vertex_pos.w, 0.0001)
            self.v_misc = vec4(
                clamp(attr.y, 0.0, 1.0),
                ch.w,
                clamp(ndc.x * 0.5 + 0.5, 0.0, 1.0),
                clamp(0.5 - ndc.y * 0.5, 0.0, 1.0)
            )
            return self.vertex_pos
        }

        pixel: fn() {
            let n = normalize(self.v_normal)
            let key = normalize(vec3(0.4, 0.75, 0.5))
            let lit = 0.40 + 0.60 * abs(dot(n, key))
            let c = self.fx_color(self.v_misc.x, self.v_attr, n, self.v_world)
            // Bar ends fall off — the slab reads as an object, not a stripe.
            let ends = 0.62 + 0.38 * smoothstep(
                0.0, 0.05,
                min(self.v_attr.y, 1.0 - self.v_attr.y)
            )
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let cam_pos = cam.xyz / max(cam.w, 0.0001)
            let d = length(self.v_world - cam_pos)
            let fogf = exp(0.0 - d * self.fog.x)
            // CONTENT: the slabs turn to POLISHED METAL and mirror the
            // channel video. A mirror-direction env map (the first pass)
            // smeared a whole frame across each bar; sampling at the
            // fragment's own SCREEN position, nudged by the normal, makes
            // each bar reflect the slice of the picture behind it — the
            // dancing stack cuts the video into venetian-blind bands and
            // reassembles it. The copper band keeps shaping the light, so
            // the metal identity stays primary.
            // fog.z = the pre-gated `content` strength.
            let cm = self.fog.z
            let suv = clamp(
                vec2(self.v_misc.z, self.v_misc.w) + vec2(n.x, 0.0 - n.y) * 0.06,
                vec2(0.0, 0.0),
                vec2(1.0, 1.0)
            )
            let env = self.tex0.sample_as_bgra(suv)
            let metal = c.xyz.mix(
                env.xyz * (0.55 + 0.75 * lit),
                clamp(cm * 1.2, 0.0, 1.0)
            )
            let rgb = (metal * (lit * ends * self.fog.y)
                + self.col_c.xyz * self.v_misc.y * 0.8)
                .mix(self.col_bg.xyz, 1.0 - fogf)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxCopper {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = amplitude gain, p1 = thickness pump, p2 free, p3 = mode blend.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (mode pack a+8b, thickness, span, beat_rate).
    #[live(vec4(0.0, 0.42, 6.5, 1.0))]
    pub shape: Vec4f,
    /// (amplitude, weave, metal, drop).
    #[live(vec4(1.6, 1.2, 3.0, 7.0))]
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
    fn copper_build_counts_and_channels() {
        let mut e = CopperEngine::new(CopperConfig { bars: 16, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(e.placed, 16);
        assert_eq!(mesh.vertex_count(), 16 * 24);
        assert_eq!(mesh.triangle_count(), 16 * 12);
        let floats = super::super::mesh::VERT_FLOATS;
        for (i, bar) in mesh.verts.chunks(floats * 24).enumerate() {
            assert_eq!(bar[3], i as f32, "a_id must be the bar index");
            let bar01 = bar[11];
            assert!((0.0..=1.0).contains(&bar01), "a_r1 must be bar01");
            for v in bar.chunks(floats) {
                assert!(
                    (v[7] - 0.0).abs() < 1e-6 || (v[7] - 1.0).abs() < 1e-6,
                    "a_aux must be the local-y gradient axis 0/1 at corners"
                );
                assert!((0.0..=1.0).contains(&v[8]), "uv.x must be x01");
                for f in v {
                    assert!(f.is_finite(), "non-finite vertex data");
                }
            }
        }
    }

    #[test]
    fn copper_bar01_is_monotone() {
        let mut e = CopperEngine::new(CopperConfig { bars: 24, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        let mut last = -1.0f32;
        for bar in mesh.verts.chunks(floats * 24) {
            assert!(bar[11] > last, "bar01 must increase with the index");
            last = bar[11];
        }
    }

    #[test]
    fn copper_mode_pack_roundtrips() {
        for (a, b) in [
            (CopperMode::Sine, CopperMode::Scissor),
            (CopperMode::Pile, CopperMode::Curtain),
            (CopperMode::Curtain, CopperMode::Sine),
        ] {
            let e = CopperEngine::new(CopperConfig { mode: a, mode_b: b, ..Default::default() });
            let pack = e.uniforms().shape.x;
            let mb = (pack / 8.0).floor();
            let ma = pack - mb * 8.0;
            assert_eq!(ma as i32, a as i32, "mode a survives the pack");
            assert_eq!(mb as i32, b as i32, "mode b survives the pack");
        }
    }

    #[test]
    fn copper_degenerate_params_stay_safe() {
        let mut e = CopperEngine::new(CopperConfig {
            bars: 0,
            width: f32::NAN,
            span: -4.0,
            thickness: 0.0,
            depth: f32::INFINITY,
            amplitude: f32::NAN,
            metal: -1.0,
            drop: f32::NAN,
            rate: 0.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0, "clamped bars must still build");
        for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "degenerate cfg leaked non-finite data");
            }
        }
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w]
        {
            assert!(v.is_finite(), "uniform not sanitized");
        }
        assert!(u.shape.w >= 0.05, "rate clamped");
    }
}
