//! Domino Liturgy — thousands of dominoes toppling in tempo.
//!
//! The whole run is a STATIC mesh: at load, domino positions are laid along
//! a generated path (spiral / serpentine / branching tree) and each tile's
//! box is emitted in its own LOCAL frame with its ground pivot and yaw baked
//! onto the vertex stream. Every frame the vertex shader computes ONE
//! number — the topple front `beat * per_beat` — and each domino's tilt is
//! a smooth clamped function of `front − arc`, so the fall travels the path
//! at exactly `per_beat` dominoes per beat. With `resurrect` on, the front
//! is a triangle wave: at the end of the run it sweeps BACK and the whole
//! liturgy rises in reverse.
//!
//! Tree layouts continue the arc count into their branches, so a branch
//! topples in parallel with the main line the moment the front crosses its
//! junction — pure per-vertex math, no per-branch state.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   geom_pos = LOCAL box corner (x lateral, y up, z 0..−thickness; the
//!              pivot edge is the y=0/z=0 line the tile tips over)
//!   normal   = LOCAL face normal (lighting after tilt+yaw rotation)
//!   a_id     = branch id (0 = main path)
//!   a_aux    = arc index along the path (absolute, in dominoes)
//!   uv       = ground pivot (x, z)
//!   a_r0     = per-domino hash, a_r1 = yaw (radians)
//!
//! # Document keys (`engine: "domino"`)
//! `layout` ("spiral" | "serpent" | "tree"), `count` (900, ≤6000),
//! `per_beat` (4 — dominoes per beat; 4 = sixteenths in 4/4 at beat_rate 1),
//! `spacing` (0.30), `tile_h`/`tile_w`/`tile_t` (0.62/0.34/0.085),
//! `branches` (5, tree only), `jitter` (0.12), `resurrect` (1),
//! `pause_beats` (2 — rest at the run's end before reversing),
//! `flash` (1.2 — impact glow gain). Bindings: `p0` nudges the front (in
//! beats — scratch the run), `p1` adds impact-flash gain, `p2` adds
//! anticipation glow (the pins ahead of the front breathe on the pulse).

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

#[derive(Clone, Copy, PartialEq)]
pub enum DominoLayout {
    Spiral,
    Serpent,
    Tree,
}

impl DominoLayout {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "spiral" => Self::Spiral,
            "serpent" | "serpentine" | "snake" => Self::Serpent,
            "tree" | "branches" => Self::Tree,
            _ => return None,
        })
    }
}

pub struct DominoConfig {
    pub layout: DominoLayout,
    pub count: usize,
    /// Dominoes toppled per beat (the musical clock of the run).
    pub per_beat: f32,
    /// Path distance between dominoes.
    pub spacing: f32,
    pub tile_h: f32,
    pub tile_w: f32,
    pub tile_t: f32,
    /// Branch count (tree layout).
    pub branches: usize,
    /// Yaw/tilt jitter 0..1.
    pub jitter: f32,
    /// 1 = the front ping-pongs (reverse resurrection); 0 = loop + snap.
    pub resurrect: f32,
    /// Beats of stillness at the end of the run before reversing.
    pub pause_beats: f32,
    /// Impact flash gain.
    pub flash: f32,
    pub seed: u64,
}

impl Default for DominoConfig {
    fn default() -> Self {
        Self {
            layout: DominoLayout::Spiral,
            count: 900,
            per_beat: 4.0,
            spacing: 0.30,
            tile_h: 0.62,
            tile_w: 0.34,
            tile_t: 0.085,
            branches: 5,
            jitter: 0.12,
            resurrect: 1.0,
            pause_beats: 2.0,
            flash: 1.2,
            seed: 47,
        }
    }
}

struct Placement {
    x: f32,
    z: f32,
    yaw: f32,
    arc: f32,
    branch: f32,
}

pub struct DominoEngine {
    pub cfg: DominoConfig,
    pub(crate) built: bool,
    /// Path radius for the camera default (set at build).
    pub extent: f32,
    /// Dominoes actually placed (arc length of the run).
    pub placed: usize,
    /// Highest arc index in the run. NOT the placed count: tree branches
    /// share arc indices with the trunk (parallel toppling), so the cycle
    /// length must follow the furthest ARC or the run sits dead for the
    /// difference before resurrecting.
    pub max_arc: f32,
}

impl DominoEngine {
    pub fn new(cfg: DominoConfig) -> Self {
        let max_arc = (cfg.count.clamp(32, 6000) - 1) as f32;
        Self { cfg, built: false, extent: 8.0, placed: 0, max_arc }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    fn layout_positions(&self, rng: &mut FxRng) -> Vec<Placement> {
        let count = self.cfg.count.clamp(32, 6000);
        let spacing = Self::san(self.cfg.spacing, 0.30).clamp(0.08, 2.0);
        let tile_h = Self::san(self.cfg.tile_h, 0.62).clamp(0.1, 3.0);
        let tile_w = Self::san(self.cfg.tile_w, 0.34).clamp(0.05, 2.0);
        let jitter = Self::san(self.cfg.jitter, 0.12).clamp(0.0, 1.0);
        let mut out = Vec::with_capacity(count);
        match self.cfg.layout {
            DominoLayout::Spiral => {
                // Archimedean spiral, arc-length-even placement. Ring gap
                // clears a fallen tile (height) so runs never interpenetrate.
                let ring_gap = (tile_h * 1.35).max(0.4);
                let b = ring_gap / std::f32::consts::TAU;
                let r0 = 1.3f32;
                let mut theta = 0.0f32;
                for i in 0..count {
                    let r = r0 + b * theta;
                    let (s, c) = theta.sin_cos();
                    // Tangent of (r(θ)cosθ, r(θ)sinθ), r' = b.
                    let hx = b * c - r * s;
                    let hz = b * s + r * c;
                    let hl = (hx * hx + hz * hz).sqrt().max(1e-4);
                    let yaw = (hx / hl).atan2(hz / hl)
                        + (rng.next_f32() - 0.5) * jitter * 0.5;
                    out.push(Placement {
                        x: c * r,
                        z: s * r,
                        yaw,
                        arc: i as f32,
                        branch: 0.0,
                    });
                    theta += spacing / r.max(0.4);
                }
            }
            DominoLayout::Serpent => {
                // Boustrophedon walker: straight rows joined by U-turns,
                // sized so the field comes out roughly square.
                let row_gap = (tile_w * 3.2).max(0.5);
                let total = count as f32 * spacing;
                let side = (total * row_gap).sqrt().clamp(4.0, 60.0);
                let mut pos = vec2f(-side * 0.5, -side * 0.4);
                let mut heading = vec2f(1.0, 0.0);
                let mut straight_left = side;
                let mut turning = 0.0f32; // radians of turn remaining
                let mut turn_dir = 1.0f32;
                let turn_r = row_gap * 0.5;
                for i in 0..count {
                    let yaw = heading.x.atan2(heading.y)
                        + (rng.next_f32() - 0.5) * jitter * 0.5;
                    out.push(Placement {
                        x: pos.x,
                        z: pos.y,
                        yaw,
                        arc: i as f32,
                        branch: 0.0,
                    });
                    if turning > 0.0 {
                        let dphi = (spacing / turn_r.max(0.1)).min(turning) * turn_dir;
                        let (sp, cp) = dphi.sin_cos();
                        heading = vec2f(
                            heading.x * cp - heading.y * sp,
                            heading.x * sp + heading.y * cp,
                        );
                        turning -= dphi.abs();
                        if turning <= 0.0 {
                            straight_left = side;
                            turn_dir = -turn_dir;
                        }
                    } else {
                        straight_left -= spacing;
                        if straight_left <= 0.0 {
                            turning = std::f32::consts::PI;
                        }
                    }
                    pos = pos + heading * spacing;
                }
            }
            DominoLayout::Tree => {
                // A SHORT trunk with branches that peel off sideways and COIL
                // (strong per-step curvature) — the garden stays compact
                // instead of sprawling into the fog. Branch arcs CONTINUE the
                // trunk's count, so a branch topples in step with the main
                // line the moment the front crosses its junction.
                let branches = self.cfg.branches.clamp(1, 12);
                let trunk_n = (count / 6).clamp(24, 90).min(count);
                let trunk_len = trunk_n as f32 * spacing;
                for i in 0..trunk_n {
                    let z = i as f32 * spacing - trunk_len * 0.5;
                    out.push(Placement {
                        x: (rng.next_f32() - 0.5) * jitter * 0.1,
                        z,
                        yaw: (rng.next_f32() - 0.5) * jitter * 0.4,
                        arc: i as f32,
                        branch: 0.0,
                    });
                }
                let left = count - trunk_n;
                let per_branch = (left / branches).max(4);
                for bi in 0..branches {
                    let jn = ((bi + 1) * trunk_n / (branches + 1)).min(trunk_n - 1);
                    let jz = jn as f32 * spacing - trunk_len * 0.5;
                    let side = if bi % 2 == 0 { 1.0f32 } else { -1.0 };
                    // Head outward, then coil back around a 3.2–6 unit
                    // radius — each branch becomes a petal of the garden.
                    let mut ang = side * rng.range(0.9, 1.4);
                    let curve = -side * spacing / rng.range(3.2, 6.0);
                    let mut px = side * 0.55;
                    let mut pz = jz;
                    for k in 0..per_branch {
                        let (sa, ca) = ang.sin_cos();
                        out.push(Placement {
                            x: px,
                            z: pz,
                            yaw: ang + (rng.next_f32() - 0.5) * jitter * 0.4,
                            arc: (jn + 1 + k) as f32,
                            branch: (bi + 1) as f32,
                        });
                        px += sa * spacing;
                        pz += ca * spacing;
                        ang += curve;
                    }
                }
            }
        }
        out
    }

    /// Static build: one 6-face box per domino, all in local frame + baked
    /// pivot/yaw/arc — the shader owns every moving part.
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let mut rng = FxRng::new(self.cfg.seed);
        let placements = self.layout_positions(&mut rng);
        let w2 = Self::san(self.cfg.tile_w, 0.34).clamp(0.05, 2.0) * 0.5;
        let h = Self::san(self.cfg.tile_h, 0.62).clamp(0.1, 3.0);
        let t = Self::san(self.cfg.tile_t, 0.085).clamp(0.01, 0.5);
        // Local corners: x ±w2, y 0..h, z 0..−t (pivot edge at y=0, z=0 —
        // the front bottom edge the tile tips over).
        let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
            // front (+z, the fall direction side)
            ([0.0, 0.0, 1.0], [[-w2, 0.0, 0.0], [w2, 0.0, 0.0], [w2, h, 0.0], [-w2, h, 0.0]]),
            // back (−z)
            ([0.0, 0.0, -1.0], [[w2, 0.0, -t], [-w2, 0.0, -t], [-w2, h, -t], [w2, h, -t]]),
            // right (+x)
            ([1.0, 0.0, 0.0], [[w2, 0.0, 0.0], [w2, 0.0, -t], [w2, h, -t], [w2, h, 0.0]]),
            // left (−x)
            ([-1.0, 0.0, 0.0], [[-w2, 0.0, -t], [-w2, 0.0, 0.0], [-w2, h, 0.0], [-w2, h, -t]]),
            // top (+y)
            ([0.0, 1.0, 0.0], [[-w2, h, 0.0], [w2, h, 0.0], [w2, h, -t], [-w2, h, -t]]),
            // bottom (−y) — visible mid-fall and during resurrection
            ([0.0, -1.0, 0.0], [[-w2, 0.0, -t], [w2, 0.0, -t], [w2, 0.0, 0.0], [-w2, 0.0, 0.0]]),
        ];
        let mut extent = 1.0f32;
        let mut max_arc = 0.0f32;
        for p in &placements {
            let hash = rng.next_f32();
            extent = extent.max((p.x * p.x + p.z * p.z).sqrt());
            max_arc = max_arc.max(p.arc);
            for (n, corners) in &faces {
                let normal = vec3f(n[0], n[1], n[2]);
                let mut ids = [0u32; 4];
                for (k, c) in corners.iter().enumerate() {
                    ids[k] = mesh.push_vert(
                        vec3f(c[0], c[1], c[2]),
                        p.branch,
                        normal,
                        p.arc,
                        vec2f(p.x, p.z),
                        hash,
                        p.yaw,
                    );
                }
                mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
            }
        }
        self.extent = extent + h;
        self.placed = placements.len();
        self.max_arc = max_arc;
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        let per_beat = san(self.cfg.per_beat, 4.0).clamp(0.25, 32.0);
        let spacing = san(self.cfg.spacing, 0.30).clamp(0.08, 2.0);
        let tile_h = san(self.cfg.tile_h, 0.62).clamp(0.1, 3.0);
        let tile_w = san(self.cfg.tile_w, 0.34).clamp(0.05, 2.0);
        // Rest tilt: the contact angle against the next tile plus settle,
        // capped short of flat so the run keeps sculptural depth.
        let tilt_max = ((spacing / tile_h).min(0.95).asin() + 0.55).clamp(0.5, 1.45);
        // Tile dims packed into one uniform slot for the pixel shader's
        // face-coordinate reconstruction (pips): h*100 in the tens.
        let dims_pack = (tile_h * 100.0).round() * 10.0 + tile_w.clamp(0.0, 9.9);
        // The topple takes ~1.7 dominoes' worth of front travel.
        let fall_len = 1.7;
        EngineUniforms {
            shape: vec4(
                per_beat,
                tilt_max,
                self.max_arc + fall_len + 1.0,
                if san(self.cfg.resurrect, 1.0) >= 0.5 { 1.0 } else { 0.0 },
            ),
            flow: vec4(
                fall_len,
                san(self.cfg.pause_beats, 2.0).clamp(0.0, 64.0) * per_beat,
                san(self.cfg.flash, 1.2).clamp(0.0, 8.0),
                dims_pack,
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Domino Liturgy: solid depth-tested boxes. Per vertex: rebuild the
    // world position from (local corner, pivot, yaw) with a tilt that is a
    // smooth clamped function of (front − arc); the front is a saw (loop)
    // or triangle (resurrect) wave of beat * per_beat. Impact flash and
    // approach anticipation are pure functions of the same difference.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxDomino = set_type_default() do #(DrawVjFxDomino::script_shader(vm)){
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
        // Content projection footprint: the layout's xz half-extent
        // (published per frame by the view from `DominoEngine::extent`).
        u_area: uniform(10.0)
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_color: varying(vec4f)
        v_face: varying(vec4f)

        // Document hook: per-tile tint. attr = (branch, arc, hash, yaw).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let mix_t = clamp(attr.x * 0.23 + attr.z * 0.12, 0.0, 1.0)
            return self.col_a.mix(self.col_b, mix_t)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            let arc = attr.y
            let pvt = self.geom.geom_uv
            let per = max(self.shape.x, 0.05)
            let cyc = max(self.shape.z, 2.0) + self.flow.y
            // Front in dominoes; p0 nudges it in beats (scratch the run).
            let fr0 = (self.time_beat.y + self.user.x) * per
            let m2 = modf(fr0, 2.0 * cyc)
            let tri_front = cyc - abs(m2 - cyc)
            let saw_front = modf(fr0, cyc)
            let front = mix(saw_front, tri_front, step(0.5, self.shape.w))
            let fall_len = max(self.flow.x, 0.25)
            let prog = clamp((front - arc) / fall_len, 0.0, 1.0)
            let ease = prog * prog * (3.0 - 2.0 * prog)
            let tilt = self.shape.y * ease
            // Rotate the local corner about the pivot edge (x axis), then yaw.
            let ct = cos(tilt)
            let st = sin(tilt)
            let lx = self.geom.geom_pos.x
            let ly = self.geom.geom_pos.y
            let lz = self.geom.geom_pos.z
            let y2 = ly * ct - lz * st
            let z2 = ly * st + lz * ct
            let cy = cos(attr.w)
            let sy = sin(attr.w)
            let pos = vec3(
                pvt.x + lx * cy + z2 * sy,
                y2,
                pvt.y - lx * sy + z2 * cy
            )
            let ny2 = self.geom.geom_normal.y * ct - self.geom.geom_normal.z * st
            let nz2 = self.geom.geom_normal.y * st + self.geom.geom_normal.z * ct
            let n = vec3(
                self.geom.geom_normal.x * cy + nz2 * sy,
                ny2,
                nz2 * cy - self.geom.geom_normal.x * sy
            )
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            // Impact flash: a window around the landing moment, sized in
            // BEATS (land/per) so fast runs carry a comet-trail of light.
            let landb = (front - arc - fall_len) / per
            let flash = exp(0.0 - landb * landb * 9.0)
                * self.flow.z * clamp(1.0 + self.user.y, 0.0, 8.0)
            // Anticipation: pins ahead of the line glow on the pulse as the
            // front approaches (within ~a beat of dominoes).
            let ahead = max(arc - front, 0.0)
            let antic = exp(0.0 - ahead / max(per, 1.0))
                * clamp(0.45 + self.user.z, 0.0, 4.0)
                * (0.35 + 0.65 * self.time_beat.w)
            let tile = self.fx_color(ease, attr, n, world.xyz)
            // CONTENT COUPLING: the run catches the live input0 as if
            // projected straight down (world xz over the layout footprint
            // → uv). Standing tiles take a whisper on their flanks;
            // toppled tiles turn face-up and ASSEMBLE the video into a
            // floor mosaic as the front travels — and resurrect it away in
            // reverse. Impact flash and pips stay untouched. fog.z is
            // host-pre-gated to 0 without real content (classic look).
            let mut tile_rgb = tile.xyz
            let cmix = self.has_content * self.fog.z
            if cmix > 0.001 {
                let ext = max(self.u_area, 1.0)
                let cuv = clamp(
                    vec2(pos.x, pos.z) / (ext * 2.0) + vec2(0.5, 0.5),
                    vec2(0.0, 0.0),
                    vec2(1.0, 1.0)
                )
                let texel = self.tex0.sample_nearest(cuv, 0.0)
                // Flanks catch a good half of it, faces the lot: the mosaic
                // only reads as a picture if the standing run already
                // carries it, not just the toppled tail.
                let catch = 0.55 + 0.45 * clamp(n.y, 0.0, 1.0)
                tile_rgb = mix(
                    tile_rgb,
                    texel.xyz * 1.35,
                    clamp(cmix * 1.6, 0.0, 1.0) * catch
                )
            }
            let key = normalize(vec3(0.45, 0.8, 0.35))
            let lit = 0.42 + 0.58 * clamp(dot(n, key), 0.0, 1.0)
            let dim = 1.0 - 0.30 * ease
            let emiss = self.col_c.xyz * (flash + antic * 0.5)
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let d = length(world.xyz - cam.xyz / max(cam.w, 0.0001))
            let fogf = exp(0.0 - d * self.fog.x)
            let rgb = (tile_rgb * (lit * dim * self.fog.y) + emiss)
                .mix(self.col_bg.xyz, 1.0 - fogf)
            self.v_color = vec4(rgb, clamp(flash, 0.0, 1.0))
            // Face coordinates for the pixel pips: dims unpacked from the
            // packed uniform (h*100 in the tens, w in the ones).
            let pack = self.flow.w
            let h100 = floor(pack / 10.0)
            let tw = max(pack - h100 * 10.0, 0.02)
            let th = max(h100 / 100.0, 0.05)
            let fu = clamp(lx / tw + 0.5, 0.0, 1.0)
            let fv = clamp(ly / th, 0.0, 1.0)
            self.v_face = vec4(fu, fv, abs(self.geom.geom_normal.z), attr.z)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let mut rgb = self.v_color.xyz
            if self.v_face.z > 0.7 {
                // Big faces: divider line + a per-tile random pip pattern
                // (2 cols x 3 rows per face); pips flare with the impact.
                let line = 1.0 - smoothstep(0.0, 0.035, abs(self.v_face.y - 0.5))
                let cell = vec2(self.v_face.x * 2.0, self.v_face.y * 3.0)
                let ci = floor(cell)
                let show = step(
                    0.42,
                    fract(sin((ci.x + ci.y * 2.0 + self.v_face.w * 61.0) * 12.9898) * 43758.5453)
                )
                let d = length(fract(cell) - vec2(0.5, 0.5))
                let pip = (1.0 - smoothstep(0.14, 0.26, d)) * show
                rgb = rgb * (1.0 - pip * 0.55 - line * 0.35)
                    + self.col_c.xyz * (pip * self.v_color.w)
            }
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
pub struct DrawVjFxDomino {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = front nudge (beats), p1 = flash gain add, p2 = anticipation add.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (per_beat, tilt_max, cycle arcs, resurrect).
    #[live(vec4(4.0, 1.1, 900.0, 1.0))]
    pub shape: Vec4f,
    /// (fall_len, pause dominoes, flash gain, dims pack).
    #[live(vec4(1.7, 8.0, 1.2, 620.34))]
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
    fn domino_build_all_layouts_finite() {
        for layout in [DominoLayout::Spiral, DominoLayout::Serpent, DominoLayout::Tree] {
            let mut e = DominoEngine::new(DominoConfig {
                layout,
                count: 300,
                ..Default::default()
            });
            let mut mesh = FxMesh::default();
            e.build(&mut mesh);
            assert!(e.placed >= 300 - 12, "placed {} of 300", e.placed);
            assert_eq!(mesh.vertex_count(), e.placed * 24);
            assert_eq!(mesh.triangle_count(), e.placed * 12);
            assert!(e.extent.is_finite() && e.extent > 1.0);
            for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
                for f in v {
                    assert!(f.is_finite(), "non-finite vertex data");
                }
            }
        }
    }

    #[test]
    fn domino_arcs_monotone_on_main_path() {
        let mut e = DominoEngine::new(DominoConfig {
            layout: DominoLayout::Spiral,
            count: 200,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        // One vertex per domino (every 24): arc must count 0,1,2,…
        let floats = super::super::mesh::VERT_FLOATS;
        for (i, dom) in mesh.verts.chunks(floats * 24).enumerate() {
            assert_eq!(dom[7], i as f32, "arc index must be the domino order");
        }
    }

    #[test]
    fn domino_tree_cycle_follows_max_arc_not_count() {
        let mut e = DominoEngine::new(DominoConfig {
            layout: DominoLayout::Tree,
            count: 800,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        // Branches share arc indices with the trunk (parallel toppling), so
        // the furthest arc is far below the placed count…
        assert!(
            e.max_arc + 1.0 < e.placed as f32 * 0.6,
            "tree arcs must run in parallel: max_arc {} placed {}",
            e.max_arc,
            e.placed
        );
        // …and the resurrection cycle must follow the ARC, or the run sits
        // dead for the difference.
        let u = e.uniforms();
        assert!(u.shape.z < e.placed as f32 * 0.6, "cycle must follow the furthest arc");
    }

    #[test]
    fn domino_degenerate_params_stay_safe() {
        let mut e = DominoEngine::new(DominoConfig {
            count: 0,
            per_beat: f32::NAN,
            spacing: -3.0,
            tile_h: 0.0,
            tile_t: f32::INFINITY,
            pause_beats: -9.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0);
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w] {
            assert!(v.is_finite(), "uniform not sanitized");
        }
        assert!(u.shape.y >= 0.5 && u.shape.y <= 1.45, "tilt_max clamped");
    }
}
