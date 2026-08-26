//! Tiles — the input texture shattered into a grid of textured-quad
//! particles. Every tile is one quad carrying its own window into input0
//! plus grid coordinates and seeds on the vertex stream; the vertex shader
//! runs one of four endless motion programs (wave / shatter / conveyor /
//! spiral) off time + beat uniforms. Static mesh: the CPU uploads the grid
//! exactly once, then every frame is pure shader work.
//!
//! This engine has ITS OWN shader (`DrawVjFxTiles`) because the pixel stage
//! samples input0 per tile — and thanks to the runtime's animated dummy
//! input, it renders real content even with nothing bound.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   geom_pos = tile REST CENTRE on the image plane (all 4 corners share it)
//!   a_id     = tile index, row-major from bottom-left (gx = id % grid,
//!              gy = id / grid — the shader derives grid coords AND the
//!              tile's uv window from this one number)
//!   normal   = shatter flight vector: unit direction * flight distance,
//!              outward from the plane centre + toward the camera (BAKED,
//!              rim tiles fly further)
//!   a_aux    = radial distance from plane centre 0..1 (wave/spiral phase)
//!   uv       = corner 0/1 — tile-local uv AND the window corner
//!   a_r0     = per-tile random (shatter stagger, shade jitter)
//!   a_r1     = per-tile random (tumble direction/speed)
//!
//! # Motion programs (`mode`, u_shape.x)
//!   wave     0 — traveling water swell; tiles tilt with the wave slope,
//!               amplitude pumps on the beat
//!   shatter  1 — BAR-SYNCED: the picture explodes along the baked flight
//!               vectors over the first third of the bar, hangs tumbling,
//!               and reassembles pixel-perfect before the next downbeat
//!   conveyor 2 — endless belt: alternate rows stream opposite directions,
//!               wrap at the edges (tiles roll away over the lip), lanes
//!               flash in sequence on the beat
//!   spiral   3 — differential whirlpool: inner tiles orbit faster and sink
//!               into a beat-breathing funnel
//!   hook     4 — THE ENGINE CONTRIBUTES NO MOTION: every tile sits at its
//!               rest centre and the document's `fx_tile` / `fx_tile_spin`
//!               vertex hooks own the whole choreography
//!
//! # Document keys (`engine: "tiles"`)
//! `grid` (24, 4..64 tiles per side), `spread` (7 plane width),
//! `aspect` (1.0 plane height = spread*aspect), `gap` (0.06 grout),
//! `amp` (0.5 wave/lift amplitude), `freq` (1.0 — wave frequency, conveyor
//! speed, spiral angular speed), `spin` (1.0 shatter tumble), `scatter`
//! (1.2 shatter flight distance, in spreads), `extrude` (0.0 — per-tile
//! LUMA RELIEF: the tile's centre texel pushes the tile along the plane
//! normal by this many world units and shades it with its own height, so
//! the picture stands up off the wall). Camera: the engine flies its
//! own gently swaying front-on rig (doc cam keys are ignored, like tunnel).
//! Bindings: `p0` ADDS shatter drive (strobe the explosion), `p1` scales
//! wave amplitude, `p2` adds grout glow.
//! Hooks: `fx_color(t, attr, content, cmix)` (pixel — the look), plus the
//! per-tile MOTION pair, applied on top of whatever the mode did:
//! `fx_tile(id, grid, uv_window, t) -> (dx, dy, dz, scale)` and
//! `fx_tile_spin(id, grid, uv_window, t) -> (axis.xyz, angle)`.

use super::engines::{CamPose, EngineUniforms};
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

#[derive(Clone, Copy, PartialEq)]
pub enum TilesMode {
    Wave = 0,
    Shatter = 1,
    Conveyor = 2,
    Spiral = 3,
    /// No engine motion at all — the document's `fx_tile` /
    /// `fx_tile_spin` vertex hooks are the whole choreography.
    Hook = 4,
}

impl TilesMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "wave" | "ocean" => Self::Wave,
            "shatter" => Self::Shatter,
            "conveyor" | "belt" => Self::Conveyor,
            "spiral" | "whirl" => Self::Spiral,
            "hook" | "plane" => Self::Hook,
            _ => return None,
        })
    }
}

pub struct TilesConfig {
    pub mode: TilesMode,
    /// Tiles per side (square grid).
    pub grid: usize,
    /// Plane width in world units.
    pub spread: f32,
    /// Plane height = spread * aspect.
    pub aspect: f32,
    /// Grout fraction 0..0.9 — each tile shrinks by this much.
    pub gap: f32,
    /// Wave / lift amplitude.
    pub amp: f32,
    /// Mode-specific rate: wave frequency, conveyor speed, spiral spin.
    pub freq: f32,
    /// Shatter tumble amount.
    pub spin: f32,
    /// Shatter flight distance in spreads.
    pub scatter: f32,
    /// LUMA RELIEF along the plane normal, in world units (0 = flat wall).
    /// The vertex stage fetches the tile's centre texel and pushes the
    /// whole tile out by `luma * extrude` — the picture becomes a relief.
    pub extrude: f32,
    pub seed: u64,
}

impl Default for TilesConfig {
    fn default() -> Self {
        Self {
            mode: TilesMode::Wave,
            grid: 24,
            spread: 7.0,
            aspect: 1.0,
            gap: 0.06,
            amp: 0.5,
            freq: 1.0,
            spin: 1.0,
            scatter: 1.2,
            extrude: 0.0,
            seed: 9,
        }
    }
}

pub struct TilesEngine {
    pub cfg: TilesConfig,
    pub(crate) built: bool,
}

impl TilesEngine {
    pub fn new(cfg: TilesConfig) -> Self {
        Self { cfg, built: false }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    fn grid(&self) -> usize {
        self.cfg.grid.clamp(4, 64)
    }

    fn spans(&self) -> (f32, f32) {
        let span = Self::san(self.cfg.spread, 7.0).clamp(1.0, 60.0);
        let aspect = Self::san(self.cfg.aspect, 1.0).clamp(0.2, 2.0);
        (span, span * aspect)
    }

    /// Static build: one quad per tile, all four corners at the tile's rest
    /// centre; the shader expands corners through the (possibly rotated)
    /// tile frame. The shatter flight vector is baked into the normal with
    /// its DISTANCE as the length — rim tiles fly further, and the whole
    /// flight costs zero uniforms.
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let grid = self.grid();
        let (span, h_span) = self.spans();
        let scatter = Self::san(self.cfg.scatter, 1.2).clamp(0.0, 4.0);
        let half_diag = ((span * 0.5) * (span * 0.5) + (h_span * 0.5) * (h_span * 0.5))
            .sqrt()
            .max(1e-3);
        let mut rng = FxRng::new(self.cfg.seed);
        let corners = [vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0)];
        for gy in 0..grid {
            for gx in 0..grid {
                let id = (gy * grid + gx) as f32;
                let cx = ((gx as f32 + 0.5) / grid as f32 - 0.5) * span;
                let cy = ((gy as f32 + 0.5) / grid as f32 - 0.5) * h_span;
                let center = vec3f(cx, cy, 0.0);
                let rad01 = ((cx * cx + cy * cy).sqrt() / half_diag).min(1.0);
                // Flight vector: outward from centre + toward the camera,
                // jittered, distance grows with the rim + the tile's roll.
                let d = vec3f(
                    cx / half_diag * 0.9 + rng.range(-0.35, 0.35),
                    cy / half_diag * 0.9 + rng.range(-0.35, 0.35),
                    0.55 + rng.next_f32() * 0.95,
                );
                let dl = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-4);
                let dist = span * (0.30 + 0.55 * rng.next_f32() + 0.35 * rad01) * scatter;
                let flight = vec3f(d.x / dl * dist, d.y / dl * dist, d.z / dl * dist);
                let r0 = rng.next_f32();
                let r1 = rng.next_f32();
                let mut ids = [0u32; 4];
                for (k, uv) in corners.iter().enumerate() {
                    ids[k] = mesh.push_vert(center, id, flight, rad01, *uv, r0, r1);
                }
                mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
            }
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        let grid = self.grid() as f32;
        let (span, h_span) = self.spans();
        let gap = san(self.cfg.gap, 0.06).clamp(0.0, 0.9);
        EngineUniforms {
            shape: vec4(
                self.cfg.mode as i32 as f32,
                grid,
                span,
                (span / grid) * (1.0 - gap),
            ),
            flow: vec4(
                san(self.cfg.amp, 0.5).clamp(0.0, 8.0),
                san(self.cfg.freq, 1.0).clamp(0.0, 12.0),
                san(self.cfg.spin, 1.0).clamp(0.0, 8.0),
                (h_span / grid) * (1.0 - gap),
            ),
        }
    }

    /// The tiles-only instance block (`self.tile` in the shader), carried
    /// beside the shared `shape`/`flow` pair because both of those are
    /// already full: (extrude, 0, 0, 0). The three free lanes are reserved
    /// for the next tiles-only key — the layout is a contract, so a new
    /// value takes a free lane, never `.x`.
    pub fn extra(&self) -> Vec4f {
        vec4(Self::san(self.cfg.extrude, 0.0).clamp(0.0, 12.0), 0.0, 0.0, 0.0)
    }

    /// Front-on rig with a gentle parallax sway — a flat picture plane must
    /// never be orbited edge-on, so the engine owns its camera (tunnel law).
    pub fn camera(&self, time: f32) -> CamPose {
        let (span, h_span) = self.spans();
        let aspect = (h_span / span).max(0.55);
        let dist = span * (0.58 + 0.55 * aspect) * (1.0 + 0.025 * (time * 0.09).sin());
        CamPose {
            eye: vec3f(
                (time * 0.19).sin() * span * 0.05,
                (time * 0.13).sin() * span * 0.035,
                dist,
            ),
            target: vec3f(0.0, 0.0, 0.0),
            fov: 50.0,
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Tiles: opaque depth-tested textured quads. The VERTEX stage runs the
    // motion program (rest centre -> animated frame via one Rodrigues
    // rotation), derives the tile's uv window from a_id alone, and bakes
    // shade/flash into a varying; the pixel stage samples input0 and draws
    // the grout line. Four modes share one modest shader.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxTiles = set_type_default() do #(DrawVjFxTiles::script_shader(vm)){
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

        v_uv: varying(vec2f)
        v_local: varying(vec2f)
        // (shade, flash, 0, 0)
        v_shade: varying(vec4f)

        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        xcross: fn(a: vec3, b: vec3) -> vec3 {
            return vec3(
                a.y * b.z - a.z * b.y,
                a.z * b.x - a.x * b.z,
                a.x * b.y - a.y * b.x
            )
        }

        // Rodrigues rotation of v around unit axis a by (cos c, sin s).
        rodr: fn(v: vec3, a: vec3, c: float, s: float) -> vec3 {
            let axv = self.xcross(a, v)
            let ad = dot(a, v)
            return v * c + axv * s + a * (ad * (1.0 - c))
        }

        // THE LOOK — one tile fragment, doc-replaceable (CONTRACT.md).
        //   t       = the mode's highlight drive (wave crest / shatter
        //             flight / conveyor lane pulse), 0..2
        //   attr    = (per-tile shade, edge 0..1 — 1 in the middle of the
        //             tile, 0 at the grout —, stagger rnd, tumble rnd)
        //   content = input0 at THIS TILE'S uv window — the tile grid IS
        //             the picture in this family, so the classic look
        //             draws the clip whole and needs no cmix ramp
        //   cmix    = pre-gated content strength, for looks that want it
        // Returns the finished fragment colour (glow gain included).
        fx_color: fn(t: float, attr: vec4, content: vec4, cmix: float) -> vec4 {
            let border = attr.y
            let lit = content.xyz * (attr.x * (0.35 + 0.65 * border))
            let glowline = self.col_c.xyz * (1.0 - border)
                * (t + clamp(self.user.z, 0.0, 4.0))
            return vec4((lit + glowline) * self.fog.y, 1.0)
        }

        // THE MOTION — per-tile placement, VERTEX stage, doc-replaceable.
        // Runs for EVERY mode, on top of whatever the mode already did, so
        // a document can dust the stock wave with jitter or (with
        // `mode: "hook"`, which contributes nothing) own the choreography
        // outright.
        //   id        = tile index, row-major from the bottom-left
        //   grid      = this tile's (gx, gy); tiles per side = self.shape.y
        //   uv_window = the tile's window into input0, (u0, v0, u1, v1)
        //   t         = document time (self.time_beat.x)
        // Returns (dx, dy, dz, scale): a world-space offset from the tile's
        // animated centre plus a size multiplier (1.0 = the engine's tile).
        fx_tile: fn(id: float, grid: vec2, uv_window: vec4, t: float) -> vec4 {
            return vec4(0.0, 0.0, 0.0, 1.0)
        }

        // THE ORIENTATION — `fx_tile`'s companion, VERTEX stage.
        // Returns (axis.xyz, angle): an EXTRA rotation of the tile's frame,
        // composed after the mode's own. Angle 0 (the default) leaves the
        // mode's rotation exactly as it was — which is why every tiles
        // document written before this hook existed still renders the same.
        fx_tile_spin: fn(id: float, grid: vec2, uv_window: vec4, t: float) -> vec4 {
            return vec4(0.0, 0.0, 1.0, 0.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let mode = self.shape.x
            let gw = max(self.shape.y, 1.0)
            let span = max(self.shape.z, 0.001)
            let id = attr.x
            let gx = modf(id, gw)
            let gy = floor(id / gw)
            let corner = self.geom.geom_uv
            let rest = self.geom.geom_pos
            let t = self.time_beat.x
            let mut ctr = rest
            let mut axis = vec3(0.0, 0.0, 1.0)
            let mut ang = 0.0
            // Subtle per-tile shade variation keeps a flat image alive.
            let mut shade = 1.0 + (attr.z - 0.5) * 0.10
            let mut flash = 0.0
            if mode < 0.5 {
                // ---- WAVE: traveling swell, tiles tilt with the slope ----
                let k = max(self.flow.y, 0.001) * 0.9
                let amp = self.flow.x * (1.0 + self.time_beat.w * 0.55)
                    * clamp(1.0 + self.user.y, 0.0, 4.0)
                let ph = t * 1.05
                let ax = rest.x * k
                let ay = rest.y * k
                let z = sin(ax + ph) + 0.6 * sin(ay * 1.31 - ph * 0.77)
                    + 0.35 * sin((ax + ay) * 0.63 + ph * 0.51)
                ctr = rest + vec3(0.0, 0.0, z * amp)
                let dc = 0.35 * 0.63 * cos((ax + ay) * 0.63 + ph * 0.51)
                let dzdx = (cos(ax + ph) + dc) * k * amp
                let dzdy = (0.6 * 1.31 * cos(ay * 1.31 - ph * 0.77) + dc) * k * amp
                let gl = max(length(vec2(dzdx, dzdy)), 0.001)
                axis = vec3(dzdy / gl, 0.0 - dzdx / gl, 0.0)
                ang = min(gl * 0.8, 1.1)
                flash = clamp(z * 0.5 - 0.30, 0.0, 1.0)
                    * (0.10 + 0.30 * self.time_beat.w)
            } else { if mode < 1.5 {
                // ---- SHATTER: bar-synced flight along the baked vector.
                // The DOWNBEAT is the blast; the picture reassembles by
                // two-thirds of the bar and HOLDS whole until the next hit.
                let bar = self.sig.x
                let stag = attr.z * 0.10
                let b = clamp((bar - stag) / max(1.0 - stag, 0.3), 0.0, 1.0)
                let fly = smoothstep(0.0, 0.07, b)
                    * (1.0 - smoothstep(0.30, 0.68, b))
                let drive = clamp(fly + self.user.x, 0.0, 1.6)
                ctr = rest + self.geom.geom_normal * drive
                let axr = vec3(
                    self.hash1(id * 3.71 + 1.0) - 0.5,
                    self.hash1(id * 5.13 + 2.0) - 0.5,
                    self.hash1(id * 7.77 + 3.0) - 0.5 + 0.02
                )
                axis = axr / max(length(axr), 0.05)
                ang = drive * self.flow.z * (attr.w * 2.0 - 1.0) * 5.0
                flash = drive * 0.5
                shade = shade * (1.0 - drive * 0.22)
            } else { if mode > 3.5 {
                // ---- HOOK: the engine stands still. `fx_tile` /
                // `fx_tile_spin` below are the entire motion program.
                ctr = rest
            } else { if mode < 2.5 {
                // ---- CONVEYOR: alternate rows stream, wrap over the lip --
                let dirn = 1.0 - 2.0 * modf(gy, 2.0)
                let sp = self.flow.y * (0.7 + 0.6 * fract(gy * 0.371)) * span * 0.14
                let a = rest.x + dirn * modf(t * sp, span) + span * 0.5
                let x = modf(modf(a, span) + span, span) - span * 0.5
                let edge = smoothstep(0.72, 1.0, abs(x) / (span * 0.5))
                let bob = sin(t * 1.7 + gy * 1.3) * self.flow.x * 0.10
                ctr = vec3(x, rest.y, bob - edge * span * 0.16)
                axis = vec3(0.0, 1.0, 0.0)
                ang = edge * 1.2 * (step(0.0, x) * 2.0 - 1.0)
                // Lanes flash in golden-ratio sequence on the beat.
                let sel = fract(gy * 0.618034 + fract(floor(self.time_beat.y) * 0.381966))
                flash = self.time_beat.w * (1.0 - smoothstep(0.0, 0.14, sel)) * 0.6
                shade = shade * (1.0 - edge * 0.35)
            } else {
                // ---- SPIRAL: differential whirlpool with a beat funnel ---
                let rad = max(length(vec2(rest.x, rest.y)), 0.001)
                let rad01 = min(rad / (span * 0.5), 1.4)
                let w = self.flow.y * (1.55 - rad01) * 0.55
                let da = t * w
                let ca = cos(da)
                let sa = sin(da)
                let sink = (1.0 - min(rad01, 1.0)) * (1.0 - min(rad01, 1.0))
                let funnel = 0.0 - sink * self.flow.x * (1.6 + 1.2 * self.time_beat.w)
                let ripple = sin(rad01 * 5.0 - t * 1.3) * self.flow.x * 0.30
                ctr = vec3(
                    rest.x * ca - rest.y * sa,
                    rest.x * sa + rest.y * ca,
                    funnel + ripple
                )
                axis = vec3(0.0, 0.0, 1.0)
                ang = da
                flash = self.time_beat.w * 0.25 * (1.0 - min(rad01, 1.0))
            }}}}
            // ---- EXTRUDE: the tile's own texel lifts it off the wall ----
            // Gated on the key so a document that never asks for relief is
            // bit-identical to before (and pays for no fetch). The vertex
            // stage's legal sampler is sample_nearest with an explicit lod.
            let ext = self.tile.x
            if ext > 0.0001 {
                let cuv = vec2((gx + 0.5) / gw, 1.0 - (gy + 0.5) / gw)
                let texel = self.tex0.sample_nearest(cuv, 0.0)
                let lum = clamp(dot(texel.xyz, vec3(0.299, 0.587, 0.114)), 0.0, 1.0)
                ctr = ctr + vec3(0.0, 0.0, lum * ext)
                // Height shading: without it a relief reads as a flat
                // picture again the moment the camera faces it head-on.
                shade = shade * (0.62 + 0.62 * lum)
            }
            // ---- DOC-AUTHORED PER-TILE MOTION (fx_tile / fx_tile_spin) ---
            let gv = vec2(gx, gy)
            let uvw = vec4(
                gx / gw,
                1.0 - (gy + 1.0) / gw,
                (gx + 1.0) / gw,
                1.0 - gy / gw
            )
            let mv = self.fx_tile(id, gv, uvw, t)
            ctr = ctr + mv.xyz
            let tile_scale = clamp(mv.w, 0.0, 8.0)
            let c = cos(ang)
            let s = sin(ang)
            let mut p = self.rodr(vec3(1.0, 0.0, 0.0), axis, c, s)
            let mut q = self.rodr(vec3(0.0, 1.0, 0.0), axis, c, s)
            let sp = self.fx_tile_spin(id, gv, uvw, t)
            let sl = length(sp.xyz)
            if sl > 0.0001 {
                let sax = sp.xyz / sl
                let sc2 = cos(sp.w)
                let ss2 = sin(sp.w)
                p = self.rodr(p, sax, sc2, ss2)
                q = self.rodr(q, sax, sc2, ss2)
            }
            let ox = (corner.x - 0.5) * self.shape.w * tile_scale
            let oy = (corner.y - 0.5) * self.flow.w * tile_scale
            let wpos = ctr + p * ox + q * oy
            let world = self.draw_list.view_transform
                * vec4(wpos.x, wpos.y, wpos.z, 1.0)
            // The tile's window into input0, derived from id alone. gy runs
            // bottom-up in world, v runs top-down in the image.
            self.v_uv = vec2((gx + corner.x) / gw, 1.0 - (gy + corner.y) / gw)
            self.v_local = corner
            self.v_shade = vec4(shade, clamp(flash, 0.0, 2.0), attr.z, attr.w)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let c = self.tex0.sample_as_bgra(self.v_uv)
            let e = min(
                min(self.v_local.x, 1.0 - self.v_local.x),
                min(self.v_local.y, 1.0 - self.v_local.y)
            )
            let border = smoothstep(0.0, 0.10, e)
            return self.fx_color(
                self.v_shade.y,
                vec4(self.v_shade.x, border, self.v_shade.z, self.v_shade.w),
                c,
                self.fog.z
            )
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxTiles {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = shatter drive add, p1 = wave amp scale, p2 = grout glow.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (mode, grid, spread, tile width incl. gap).
    #[live(vec4(0.0, 24.0, 7.0, 0.27))]
    pub shape: Vec4f,
    /// (amp, freq, spin, tile height incl. gap).
    #[live(vec4(0.5, 1.0, 1.0, 0.27))]
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
    /// TILES-ONLY instance block — `shape`/`flow` are both full.
    /// (extrude world units, reserved, reserved, reserved).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub tile: Vec4f,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_build_counts_and_channels() {
        let mut e = TilesEngine::new(TilesConfig { grid: 12, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(mesh.vertex_count(), 12 * 12 * 4);
        assert_eq!(mesh.triangle_count(), 12 * 12 * 2);
        let floats = super::super::mesh::VERT_FLOATS;
        for (i, v) in mesh.verts.chunks(floats).enumerate() {
            let id = v[3];
            assert_eq!(id, (i / 4) as f32, "row-major tile ids");
            // Flight vector: finite, camera-ward (z > 0), non-degenerate.
            assert!(v[4].is_finite() && v[5].is_finite() && v[6].is_finite());
            assert!(v[6] > 0.0, "flight must move toward the camera");
            let dl = (v[4] * v[4] + v[5] * v[5] + v[6] * v[6]).sqrt();
            assert!(dl > 0.1, "flight distance collapsed");
            // Radial + corner channels in range.
            assert!((0.0..=1.0).contains(&v[7]), "rad01 out of range");
            assert!(v[8] == 0.0 || v[8] == 1.0, "corner u must be 0/1");
            assert!(v[9] == 0.0 || v[9] == 1.0, "corner v must be 0/1");
        }
    }

    #[test]
    fn tiles_static_and_uniforms_sane() {
        let e = TilesEngine::new(TilesConfig {
            grid: 24,
            spread: 8.0,
            aspect: 0.5,
            gap: 0.1,
            ..Default::default()
        });
        let u = e.uniforms();
        assert_eq!(u.shape.y, 24.0);
        let tw = u.shape.w;
        let th = u.flow.w;
        assert!((tw - (8.0 / 24.0) * 0.9).abs() < 1e-4);
        assert!((th - (4.0 / 24.0) * 0.9).abs() < 1e-4);
    }

    #[test]
    fn tiles_degenerate_params_stay_safe() {
        let mut e = TilesEngine::new(TilesConfig {
            grid: 0,
            spread: f32::NAN,
            aspect: -3.0,
            gap: 9.0,
            scatter: f32::INFINITY,
            amp: f32::NAN,
            freq: -1.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0);
        for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "degenerate cfg leaked non-finite data");
            }
        }
        let u = e.uniforms();
        for f in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z,
            u.flow.w]
        {
            assert!(f.is_finite(), "uniform not sanitized");
        }
        assert!(u.shape.w > 0.0 && u.flow.w > 0.0, "tile size must stay positive");
        let cam = e.camera(3.7);
        assert!(cam.eye.z.is_finite() && cam.eye.z > 0.0);
    }

    #[test]
    fn extrude_defaults_off_and_stays_sane() {
        // Default = a flat wall: the vertex stage's `ext > 0.0001` gate is
        // what makes every tiles document written before the relief
        // existed render bit-identically, so the default must be zero.
        let e = TilesEngine::new(TilesConfig::default());
        assert_eq!(e.extra().x, 0.0);
        for bad in [f32::NAN, f32::INFINITY, -5.0, 1e9] {
            let e = TilesEngine::new(TilesConfig { extrude: bad, ..Default::default() });
            let x = e.extra();
            assert!(x.x.is_finite() && (0.0..=12.0).contains(&x.x), "extrude {bad} leaked");
        }
        let e = TilesEngine::new(TilesConfig { extrude: 1.5, ..Default::default() });
        assert_eq!(e.extra().x, 1.5);
        // The three reserved lanes are part of the contract.
        let x = e.extra();
        assert_eq!((x.y, x.z, x.w), (0.0, 0.0, 0.0));
    }

    #[test]
    fn hook_mode_parses_and_carries_its_own_slot() {
        assert!(matches!(TilesMode::parse("hook"), Some(TilesMode::Hook)));
        assert!(matches!(TilesMode::parse("plane"), Some(TilesMode::Hook)));
        assert!(TilesMode::parse("nope").is_none());
        // The shader branches on `mode > 3.5`, so the slot must be 4.
        assert_eq!(TilesMode::Hook as i32, 4);
        let e = TilesEngine::new(TilesConfig { mode: TilesMode::Hook, ..Default::default() });
        assert_eq!(e.uniforms().shape.x, 4.0);
    }
}
