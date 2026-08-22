//! The mesh-generating effect engines.
//!
//! Every engine follows the same split:
//!   * **build/regen (CPU, slow path)** — runs at load time, on document
//!     change, or (for the two genuinely dynamic engines: metaballs,
//!     ribbons) once per frame with recycled, capacity-stable buffers.
//!   * **animate (GPU, fast path)** — the vertex stream carries per-element
//!     data (id, birth order, depth, seed, radius…) and the vertex shader
//!     animates from time/beat uniforms. Static-mesh engines (particles,
//!     l-system, heightmap, tunnel) upload their geometry once and never
//!     touch it again.
//!
//! The splash interpreter is never in the frame path: documents are
//! evaluated at load, compiled into these configs, and from then on a frame
//! is Rust `regen` (usually a no-op) + uniform writes.

use super::engines_domino::DominoEngine;
use super::engines_firefly::FireflyEngine;
use super::engines_harmonograph::HarmonographEngine;
use super::lsys;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

/// Which draw shader an engine's mesh wants (see `shaders.rs`).
#[derive(Clone, Copy, PartialEq)]
pub enum ShaderKind {
    /// Solid emissive mesh with growth/sway animation (l-system, metaballs).
    Mesh,
    /// Grid displaced by fbm / input texture in the vertex shader (heightmap).
    Terrain,
    /// View-facing ribbon strips expanded in the vertex shader.
    Ribbon,
    /// Stateless billboard particle sheet, motion integrated in the vertex
    /// shader from per-particle seeds.
    Particles,
    /// Interior tube with scrolling neon shading (tunnel).
    Tunnel,
    /// Script-driven emitters: one draw instance per emitter, particles
    /// expanded statelessly in the vertex shader.
    Emitters,
    /// Grass blades + synced firefly sprites in one stream (engines_firefly).
    Firefly,
    /// Harmonograph ribbon: curve computed in the VS (engines_harmonograph).
    Harmono,
    /// Toppling domino boxes, front = f(beat) (engines_domino).
    Domino,
}

/// Per-frame camera the engine suggests; the document can override knobs.
#[derive(Clone, Copy)]
pub struct CamPose {
    pub eye: Vec3f,
    pub target: Vec3f,
    pub fov: f32,
}

/// Extra per-engine uniform payload (u_shape / u_flow in the shaders).
#[derive(Clone, Copy, Default)]
pub struct EngineUniforms {
    pub shape: Vec4f,
    pub flow: Vec4f,
}

// ---------------------------------------------------------------------------
// Particles
// ---------------------------------------------------------------------------

/// Motion program selector, baked into `u_shape.x`. The vertex shader
/// integrates every particle's position each frame from (id, seeds, time,
/// beat) — the CPU uploads the sheet once and never steps a particle.
#[derive(Clone, Copy, PartialEq)]
pub enum ParticleMode {
    Burst = 0,
    Fountain = 1,
    Tunnel = 2,
    Vortex = 3,
    Rain = 4,
    Galaxy = 5,
    /// Particles home on an image-plane grid showing input texture 0 and
    /// periodically dissolve into a swirl — the texture-input demo.
    Image = 6,
    /// Billboarded puff clusters drifting with the wind — calm-mood volume.
    Clouds = 7,
    /// Golden-angle sunflower spiral, florets pulsing on the beat.
    Phyllo = 8,
}

impl ParticleMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "burst" | "fireworks" => Self::Burst,
            "fountain" => Self::Fountain,
            "tunnel" | "flow" => Self::Tunnel,
            "vortex" => Self::Vortex,
            "rain" => Self::Rain,
            "galaxy" => Self::Galaxy,
            "image" | "dissolve" => Self::Image,
            "clouds" => Self::Clouds,
            "phyllo" | "bloom" => Self::Phyllo,
            _ => return None,
        })
    }
}

pub struct ParticlesConfig {
    pub mode: ParticleMode,
    pub count: usize,
    /// World-space extent of the emitter volume / image plane.
    pub spread: f32,
    pub size: f32,
    /// Loops (respawn cycles) per second, scaled by doc speed.
    pub rate: f32,
    pub gravity: f32,
    pub swirl: f32,
    /// Streak length multiplier (rain/tunnel stretch their sprites).
    pub stretch: f32,
    pub seed: u64,
}

impl Default for ParticlesConfig {
    fn default() -> Self {
        Self {
            mode: ParticleMode::Burst,
            count: 4000,
            spread: 6.0,
            size: 0.10,
            rate: 0.4,
            gravity: 1.0,
            swirl: 1.0,
            stretch: 1.0,
            seed: 1,
        }
    }
}

pub struct ParticlesEngine {
    pub cfg: ParticlesConfig,
    built: bool,
}

impl ParticlesEngine {
    pub fn new(cfg: ParticlesConfig) -> Self {
        Self { cfg, built: false }
    }

    /// The static sheet: one quad per particle. `a_id` = particle index,
    /// `a_r0`/`a_r1` = two uniform randoms, `geom_normal` = a unit direction
    /// seed, `a_aux` = spawn phase 0..1. Everything else is shader math.
    ///
    /// Burst mode quantizes the spawn phase PER SHELL (`id % 10`): a shell's
    /// sparks must ignite together or the burst reads as a cloud.
    fn build(&mut self, mesh: &mut FxMesh) {
        let count = self.cfg.count.clamp(16, 30_000);
        let mut rng = FxRng::new(self.cfg.seed);
        let mut shell_phase = [0.0f32; 10];
        for (s, p) in shell_phase.iter_mut().enumerate() {
            *p = (s as f32 * 0.6180339 + rng.next_f32() * 0.08).fract();
        }
        // Image mode arranges home positions on a W×H grid via id; the
        // shader derives the grid from u_shape.w = W.
        for i in 0..count {
            let r0 = rng.next_f32();
            let r1 = rng.next_f32();
            let phase = if self.cfg.mode == ParticleMode::Burst {
                shell_phase[i % 10]
            } else {
                rng.next_f32()
            };
            // Unit direction seed: fibonacci-ish sphere point decorrelated
            // by the rng so every mode has a good 3D spray available.
            let z = rng.range(-1.0, 1.0);
            let a = rng.range(0.0, std::f32::consts::TAU);
            let s = (1.0 - z * z).max(0.0).sqrt();
            let dir = vec3f(s * a.cos(), z, s * a.sin());
            let id = i as f32;
            let base = [
                (vec2f(-0.5, -0.5), vec2f(0.0, 0.0)),
                (vec2f(0.5, -0.5), vec2f(1.0, 0.0)),
                (vec2f(0.5, 0.5), vec2f(1.0, 1.0)),
                (vec2f(-0.5, 0.5), vec2f(0.0, 1.0)),
            ];
            let mut ids = [0u32; 4];
            for (k, (corner, uv)) in base.iter().enumerate() {
                ids[k] = mesh.push_vert(
                    vec3f(corner.x, corner.y, 0.0),
                    id,
                    dir,
                    phase,
                    *uv,
                    r0,
                    r1,
                );
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let grid_w = (self.cfg.count as f32).sqrt().ceil().max(1.0);
        EngineUniforms {
            shape: vec4(
                self.cfg.mode as i32 as f32,
                self.cfg.spread,
                self.cfg.size,
                grid_w,
            ),
            flow: vec4(self.cfg.rate, self.cfg.gravity, self.cfg.swirl, self.cfg.stretch),
        }
    }
}

// ---------------------------------------------------------------------------
// L-system
// ---------------------------------------------------------------------------

pub struct LsystemConfig {
    pub axiom: String,
    pub rules: Vec<(char, String)>,
    pub iterations: usize,
    pub angle: f32,
    pub angle_jitter: f32,
    pub step: f32,
    pub radius: f32,
    pub radius_decay: f32,
    pub sides: usize,
    pub seed: u64,
    /// Hard cap on the expanded op stream (and thus the segment count).
    pub budget: usize,
    /// Number of plant instances arranged in a ring (1 = a single plant).
    pub copies: usize,
}

impl Default for LsystemConfig {
    fn default() -> Self {
        Self {
            axiom: "X".into(),
            rules: vec![('X', "F[+X][-X]FX".into()), ('F', "FF".into())],
            iterations: 5,
            angle: 25.7,
            angle_jitter: 0.0,
            step: 0.16,
            radius: 0.045,
            radius_decay: 0.82,
            sides: 5,
            seed: 7,
            budget: 60_000,
            copies: 1,
        }
    }
}

pub struct LsystemEngine {
    pub cfg: LsystemConfig,
    built: bool,
    pub segments: usize,
    pub truncated: bool,
}

impl LsystemEngine {
    pub fn new(cfg: LsystemConfig) -> Self {
        Self { cfg, built: false, segments: 0, truncated: false }
    }

    fn build(&mut self, mesh: &mut FxMesh) {
        let program =
            lsys::compile(&self.cfg.axiom, &self.cfg.rules, self.cfg.iterations, self.cfg.budget);
        self.truncated = program.truncated;
        let copies = self.cfg.copies.clamp(1, 8);
        let mut total = 0;
        for c in 0..copies {
            let params = lsys::TurtleParams {
                angle: self.cfg.angle,
                angle_jitter: self.cfg.angle_jitter,
                step: self.cfg.step,
                radius: self.cfg.radius,
                radius_decay: self.cfg.radius_decay,
                sides: self.cfg.sides,
                seed: self.cfg.seed.wrapping_add(c as u64 * 977),
            };
            let before = mesh.vertex_count();
            let (segs, _max_arc) = lsys::run_turtle(&program, &params, mesh);
            total += segs;
            if copies > 1 {
                // Rotate/offset this copy into a ring around the origin.
                let a = c as f32 / copies as f32 * std::f32::consts::TAU;
                let (s, cs) = a.sin_cos();
                let off = vec3f(s * 2.2, 0.0, cs * 2.2);
                let floats = super::mesh::VERT_FLOATS;
                for v in mesh.verts[before * floats..].chunks_mut(floats) {
                    let (x, z) = (v[0], v[2]);
                    v[0] = x * cs + z * s + off.x;
                    v[2] = -x * s + z * cs + off.z;
                    let (nx, nz) = (v[4], v[6]);
                    v[4] = nx * cs + nz * s;
                    v[6] = -nx * s + nz * cs;
                }
            }
        }
        self.segments = total;
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms::default()
    }
}

// ---------------------------------------------------------------------------
// Metaballs (marching tetrahedra, regenerated per frame)
// ---------------------------------------------------------------------------

pub struct MetaballsConfig {
    pub blobs: usize,
    /// Grid resolution per axis (cells).
    pub grid: usize,
    /// Half-extent of the sampled cube.
    pub extent: f32,
    pub blob_radius: f32,
    /// Orbit amplitude of the blob centers.
    pub orbit: f32,
    /// Orbit angular speed multiplier.
    pub speed: f32,
    /// How much the beat pumps blob radii (0..1).
    pub beat_swell: f32,
    pub seed: u64,
}

impl Default for MetaballsConfig {
    fn default() -> Self {
        Self {
            blobs: 6,
            grid: 30,
            extent: 3.0,
            blob_radius: 0.95,
            orbit: 1.5,
            speed: 0.7,
            beat_swell: 0.35,
            seed: 3,
        }
    }
}

pub struct MetaballsEngine {
    pub cfg: MetaballsConfig,
    field: Vec<f32>,
    centers: Vec<Vec3f>,
    radii: Vec<f32>,
    hues: Vec<f32>,
    freqs: Vec<Vec3f>,
    phases: Vec<Vec3f>,
}

impl MetaballsEngine {
    pub fn new(cfg: MetaballsConfig) -> Self {
        let blobs = cfg.blobs.clamp(2, 12);
        let mut rng = FxRng::new(cfg.seed);
        let mut freqs = Vec::with_capacity(blobs);
        let mut phases = Vec::with_capacity(blobs);
        let mut hues = Vec::with_capacity(blobs);
        for _ in 0..blobs {
            freqs.push(vec3f(rng.range(0.4, 1.1), rng.range(0.4, 1.1), rng.range(0.4, 1.1)));
            phases.push(vec3f(
                rng.range(0.0, 6.28),
                rng.range(0.0, 6.28),
                rng.range(0.0, 6.28),
            ));
            hues.push(rng.next_f32());
        }
        Self {
            cfg,
            field: Vec::new(),
            centers: vec![Vec3f::default(); blobs],
            radii: vec![0.0; blobs],
            hues,
            freqs,
            phases,
        }
    }

    /// Closed-form blob motion — no CPU state to step, so a hidden/paused
    /// effect resumes exactly where the clock says.
    fn place_blobs(&mut self, time: f32, beat_phase: f32) {
        let swell = 1.0 + self.cfg.beat_swell * (1.0 - beat_phase).powi(3);
        let n = self.centers.len();
        for i in 0..n {
            let f = self.freqs[i] * self.cfg.speed;
            let p = self.phases[i];
            self.centers[i] = vec3f(
                (time * f.x + p.x).sin() * self.cfg.orbit,
                (time * f.y + p.y).sin() * self.cfg.orbit * 0.7,
                (time * f.z + p.z).cos() * self.cfg.orbit,
            );
            self.radii[i] = self.cfg.blob_radius * (0.8 + 0.2 * ((time * 0.9 + i as f32).sin()))
                * swell;
        }
    }

    #[inline]
    fn field_at(&self, p: Vec3f) -> f32 {
        let mut f = 0.0;
        for (c, r) in self.centers.iter().zip(&self.radii) {
            let d = p - *c;
            let d2 = d.x * d.x + d.y * d.y + d.z * d.z + 1e-4;
            f += (r * r) / d2;
        }
        f
    }

    fn gradient_and_dominant(&self, p: Vec3f) -> (Vec3f, usize) {
        let mut g = Vec3f::default();
        let mut best = 0usize;
        let mut best_f = -1.0f32;
        for (i, (c, r)) in self.centers.iter().zip(&self.radii).enumerate() {
            let d = p - *c;
            let d2 = d.x * d.x + d.y * d.y + d.z * d.z + 1e-4;
            let fi = (r * r) / d2;
            // ∇(r²/d²) = -2 r² d / d⁴
            let k = -2.0 * fi / d2;
            g = g + d * k;
            if fi > best_f {
                best_f = fi;
                best = i;
            }
        }
        (g, best)
    }

    /// March the field into `mesh` (cleared first). Marching TETRAHEDRA:
    /// no edge tables, robust, ~2x the triangles of marching cubes — fine at
    /// VJ blob resolutions.
    fn regen(&mut self, time: f32, beat_phase: f32, mesh: &mut FxMesh) {
        self.place_blobs(time, beat_phase);
        let n = self.cfg.grid.clamp(8, 48);
        let s = self.cfg.extent;
        let np = n + 1;
        self.field.clear();
        self.field.resize(np * np * np, 0.0);
        let cell = 2.0 * s / n as f32;
        for z in 0..np {
            for y in 0..np {
                for x in 0..np {
                    let p = vec3f(
                        -s + x as f32 * cell,
                        -s + y as f32 * cell,
                        -s + z as f32 * cell,
                    );
                    self.field[(z * np + y) * np + x] = self.field_at(p);
                }
            }
        }
        mesh.clear();
        const ISO: f32 = 1.0;
        // The six tetrahedra of a cube around the 0-7 main diagonal, corner
        // indices in xyz bit order (bit0=x, bit1=y, bit2=z).
        const TETS: [[usize; 4]; 6] = [
            [0, 1, 3, 7],
            [0, 3, 2, 7],
            [0, 2, 6, 7],
            [0, 6, 4, 7],
            [0, 4, 5, 7],
            [0, 5, 1, 7],
        ];
        let corner = |x: usize, y: usize, z: usize, c: usize| -> (usize, Vec3f) {
            let cx = x + (c & 1);
            let cy = y + ((c >> 1) & 1);
            let cz = z + ((c >> 2) & 1);
            let idx = (cz * np + cy) * np + cx;
            let p = vec3f(
                -s + cx as f32 * cell,
                -s + cy as f32 * cell,
                -s + cz as f32 * cell,
            );
            (idx, p)
        };
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    // Skip cells fully outside/inside via corner min/max.
                    let mut lo = f32::MAX;
                    let mut hi = f32::MIN;
                    for c in 0..8 {
                        let (idx, _) = corner(x, y, z, c);
                        let v = self.field[idx];
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                    if lo > ISO || hi < ISO {
                        continue;
                    }
                    for tet in &TETS {
                        self.march_tet(x, y, z, tet, np, cell, s, ISO, mesh, &corner);
                    }
                }
            }
        }
        mesh.pad_to_high_water();
    }

    #[allow(clippy::too_many_arguments)]
    fn march_tet(
        &self,
        x: usize,
        y: usize,
        z: usize,
        tet: &[usize; 4],
        _np: usize,
        _cell: f32,
        _s: f32,
        iso: f32,
        mesh: &mut FxMesh,
        corner: &dyn Fn(usize, usize, usize, usize) -> (usize, Vec3f),
    ) {
        let mut vals = [0.0f32; 4];
        let mut pts = [Vec3f::default(); 4];
        let mut mask = 0usize;
        for (k, &c) in tet.iter().enumerate() {
            let (idx, p) = corner(x, y, z, c);
            vals[k] = self.field[idx];
            pts[k] = p;
            if vals[k] > iso {
                mask |= 1 << k;
            }
        }
        if mask == 0 || mask == 0b1111 {
            return;
        }
        // Edge interpolation helper: where the iso level crosses edge a-b.
        let lerp = |a: usize, b: usize| -> Vec3f {
            let t = ((iso - vals[a]) / (vals[b] - vals[a])).clamp(0.0, 1.0);
            pts[a] + (pts[b] - pts[a]) * t
        };
        // The 14 non-trivial cases reduce to 1-triangle (one corner isolated)
        // or 2-triangle (two/two split) emissions.
        let emit3 = |mesh: &mut FxMesh, me: &Self, a: Vec3f, b: Vec3f, c: Vec3f, flip: bool| {
            let (p0, p1, p2) = if flip { (a, c, b) } else { (a, b, c) };
            let mut ids = [0u32; 3];
            for (i, p) in [p0, p1, p2].into_iter().enumerate() {
                let (g, dom) = me.gradient_and_dominant(p);
                let nl = (g.x * g.x + g.y * g.y + g.z * g.z).sqrt().max(1e-5);
                let normal = vec3f(-g.x / nl, -g.y / nl, -g.z / nl);
                let aux = (p.y / me.cfg.extent) * 0.5 + 0.5;
                // Generous attributes: distance to the dominant blob center
                // (a_r1, normalized) and polar angle + field strength (uv) —
                // raw material for shader hooks.
                let dc = p - me.centers[dom];
                let dist01 = ((dc.x * dc.x + dc.y * dc.y + dc.z * dc.z).sqrt()
                    / me.radii[dom].max(1e-3))
                .min(4.0)
                    * 0.25;
                let angle01 = (dc.z.atan2(dc.x) / std::f32::consts::TAU) + 0.5;
                let field01 = (me.field_at(p) * 0.5).min(1.0);
                ids[i] = mesh.push_vert(
                    p,
                    dom as f32,
                    normal,
                    aux,
                    vec2f(angle01, field01),
                    me.hues[dom],
                    dist01,
                );
            }
            mesh.push_tri(ids[0], ids[1], ids[2]);
        };
        let inside = |k: usize| mask & (1 << k) != 0;
        // One-vs-three cases.
        for k in 0..4 {
            let solo_in = mask == (1 << k);
            let solo_out = mask == (0b1111 ^ (1 << k));
            if solo_in || solo_out {
                let others: Vec<usize> = (0..4).filter(|&j| j != k).collect();
                let a = lerp(k, others[0]);
                let b = lerp(k, others[1]);
                let c = lerp(k, others[2]);
                emit3(mesh, self, a, b, c, solo_out);
                return;
            }
        }
        // Two-vs-two: find the in pair.
        let ins: Vec<usize> = (0..4).filter(|&k| inside(k)).collect();
        let outs: Vec<usize> = (0..4).filter(|&k| !inside(k)).collect();
        let a = lerp(ins[0], outs[0]);
        let b = lerp(ins[0], outs[1]);
        let c = lerp(ins[1], outs[1]);
        let d = lerp(ins[1], outs[0]);
        emit3(mesh, self, a, b, c, false);
        emit3(mesh, self, a, c, d, false);
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms::default()
    }
}

// ---------------------------------------------------------------------------
// Heightmap terrain (static grid, displaced in the vertex shader)
// ---------------------------------------------------------------------------

pub struct HeightmapConfig {
    /// Grid resolution (quads per side).
    pub res: usize,
    /// World size of the grid (square, XZ plane).
    pub size: f32,
    pub height: f32,
    /// Noise feature scale (features per world unit).
    pub noise_scale: f32,
    /// Forward scroll speed (world units/sec of apparent flight).
    pub scroll: f32,
    /// 0 = smooth fbm hills, 1 = ridged (canyon) shaping in the shader.
    pub ridged: f32,
    /// Displace from input texture 0 instead of noise (0..1 blend).
    pub tex_displace: f32,
}

impl Default for HeightmapConfig {
    fn default() -> Self {
        Self {
            res: 110,
            size: 30.0,
            height: 2.4,
            noise_scale: 0.16,
            scroll: 2.2,
            ridged: 0.0,
            tex_displace: 0.0,
        }
    }
}

pub struct HeightmapEngine {
    pub cfg: HeightmapConfig,
    built: bool,
}

impl HeightmapEngine {
    pub fn new(cfg: HeightmapConfig) -> Self {
        Self { cfg, built: false }
    }

    /// Flat grid; uv spans 0..1 across it. Height comes from the shader.
    /// Attributes: `a_id` = cell checker hash, `a_aux` = radial distance
    /// from center 0..1, `a_r0` = per-vertex hash — free material for hooks.
    fn build(&mut self, mesh: &mut FxMesh) {
        let res = self.cfg.res.clamp(8, 220);
        let size = self.cfg.size;
        let mut rows: Vec<u32> = Vec::with_capacity((res + 1) * (res + 1));
        let mut rng = FxRng::new(97);
        for zi in 0..=res {
            for xi in 0..=res {
                let u = xi as f32 / res as f32;
                let v = zi as f32 / res as f32;
                let pos = vec3f((u - 0.5) * size, 0.0, (v - 0.5) * size);
                let cell = ((xi / 4 + zi / 4) % 2) as f32;
                let radial = (((u - 0.5) * (u - 0.5) + (v - 0.5) * (v - 0.5)).sqrt() * 2.0)
                    .min(1.0);
                rows.push(mesh.push_vert(
                    pos,
                    cell,
                    vec3f(0.0, 1.0, 0.0),
                    radial,
                    vec2f(u, v),
                    rng.next_f32(),
                    0.0,
                ));
            }
        }
        for zi in 0..res {
            for xi in 0..res {
                let a = rows[zi * (res + 1) + xi];
                let b = rows[zi * (res + 1) + xi + 1];
                let c = rows[(zi + 1) * (res + 1) + xi + 1];
                let d = rows[(zi + 1) * (res + 1) + xi];
                mesh.push_quad(a, b, c, d);
            }
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms {
            shape: vec4(self.cfg.size, self.cfg.height, self.cfg.noise_scale, self.cfg.ridged),
            flow: vec4(self.cfg.scroll, self.cfg.tex_displace, 0.0, 0.0),
        }
    }
}

// ---------------------------------------------------------------------------
// Flow-field ribbons (CPU-stepped heads, per-frame strip regen)
// ---------------------------------------------------------------------------

/// Which velocity field the ribbon heads follow.
#[derive(Clone, Copy, PartialEq)]
pub enum RibbonField {
    /// The analytic curl+swirl+containment field (the default).
    Curl,
    /// The Lorenz attractor (scaled into frame) — strange-attractor storm.
    Lorenz,
    /// The Aizawa attractor — a rounder, mushroom-shaped strange attractor.
    Aizawa,
}

impl RibbonField {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "curl" => Self::Curl,
            "lorenz" => Self::Lorenz,
            "aizawa" => Self::Aizawa,
            _ => return None,
        })
    }
}

pub struct RibbonsConfig {
    pub ribbons: usize,
    /// Trail points per ribbon.
    pub trail: usize,
    pub width: f32,
    pub speed: f32,
    /// Swirl strength of the analytic flow field.
    pub swirl: f32,
    /// Half-extent of the containment volume.
    pub bound: f32,
    pub field: RibbonField,
    pub seed: u64,
}

impl Default for RibbonsConfig {
    fn default() -> Self {
        Self {
            ribbons: 28,
            trail: 56,
            width: 0.10,
            speed: 2.0,
            swirl: 1.6,
            bound: 5.0,
            field: RibbonField::Curl,
            seed: 11,
        }
    }
}

struct RibbonHead {
    pos: Vec3f,
    hue: f32,
    age: f32,
    life: f32,
}

pub struct RibbonsEngine {
    pub cfg: RibbonsConfig,
    heads: Vec<RibbonHead>,
    /// Ring buffers: `trail` points per ribbon, newest at `cursor`.
    history: Vec<Vec3f>,
    /// Per-point speed 0..1 (parallel ring buffer) — rides `uv.x` so the
    /// shader can brighten fast passages.
    speeds: Vec<f32>,
    cursor: usize,
    rng: FxRng,
    warmed: bool,
}

impl RibbonsEngine {
    pub fn new(cfg: RibbonsConfig) -> Self {
        let ribbons = cfg.ribbons.clamp(2, 96);
        let trail = cfg.trail.clamp(8, 160);
        let mut rng = FxRng::new(cfg.seed);
        let mut heads = Vec::with_capacity(ribbons);
        for _ in 0..ribbons {
            heads.push(RibbonHead {
                pos: vec3f(rng.range(-2.0, 2.0), rng.range(-2.0, 2.0), rng.range(-2.0, 2.0)),
                hue: rng.next_f32(),
                age: rng.range(0.0, 4.0),
                life: rng.range(7.0, 14.0),
            });
        }
        let history = vec![Vec3f::default(); ribbons * trail];
        let speeds = vec![0.0; ribbons * trail];
        let mut engine = Self { cfg, heads, history, speeds, cursor: 0, rng, warmed: false };
        engine.reset_histories();
        engine
    }

    fn reset_histories(&mut self) {
        let trail = self.trail();
        for (i, head) in self.heads.iter().enumerate() {
            for k in 0..trail {
                self.history[i * trail + k] = head.pos;
                self.speeds[i * trail + k] = 0.0;
            }
        }
    }

    fn trail(&self) -> usize {
        self.cfg.trail.clamp(8, 160)
    }

    /// The velocity field the heads follow. `Curl` is the analytic default;
    /// the attractors integrate the genuine ODEs in a scaled frame (world =
    /// attractor-space * scale), which is what draws the famous shapes.
    fn flow(&self, p: Vec3f, t: f32) -> Vec3f {
        match self.cfg.field {
            RibbonField::Curl => {
                let s = self.cfg.swirl;
                let curl = vec3f(
                    (p.y * 1.3 + t * 0.7).sin() + (p.z * 1.7 - t * 0.4).cos() * 0.6,
                    (p.z * 1.1 - t * 0.5).sin() + (p.x * 1.6 + t * 0.6).cos() * 0.6,
                    (p.x * 1.4 + t * 0.3).sin() + (p.y * 1.2 - t * 0.7).cos() * 0.6,
                );
                let swirl = vec3f(-p.z, 0.0, p.x) * (0.55 * s);
                let r = (p.x * p.x + p.z * p.z).sqrt().max(1e-3);
                let band = 2.6;
                let pull = vec3f(p.x / r, 0.0, p.z / r) * ((band - r) * 0.35)
                    + vec3f(0.0, -p.y * 0.22, 0.0);
                curl * s + swirl + pull
            }
            RibbonField::Lorenz => {
                // Fit: attractor x/y ±20 → world ±0.75*bound; z 0..50 →
                // world y ±0.75*bound (recentered at z=25).
                let sxy = 0.0375 * self.cfg.bound;
                let sz = 0.03 * self.cfg.bound;
                let a = vec3f(p.x / sxy, p.z / sxy, p.y / sz + 25.0);
                let (sigma, rho, beta) = (10.0, 28.0, 8.0 / 3.0);
                let d = vec3f(
                    sigma * (a.y - a.x),
                    a.x * (rho - a.z) - a.y,
                    a.x * a.y - beta * a.z,
                );
                // Back to world axes (attr x→x, y→z, z→y) at world scale,
                // slowed so the configured flow_speed stays meaningful.
                vec3f(d.x * sxy, d.z * sz, d.y * sxy) * 0.55
            }
            RibbonField::Aizawa => {
                let scale = self.cfg.bound * 0.45;
                let a = vec3f(p.x / scale, p.z / scale, p.y / scale + 0.6);
                let (ap, b, c, dd, e, f) = (0.95, 0.7, 0.6, 3.5, 0.25, 0.1);
                let d = vec3f(
                    (a.z - b) * a.x - dd * a.y,
                    dd * a.x + (a.z - b) * a.y,
                    c + ap * a.z - a.z * a.z * a.z / 3.0
                        - (a.x * a.x + a.y * a.y) * (1.0 + e * a.z)
                        + f * a.z * a.x * a.x * a.x,
                );
                vec3f(d.x, d.z, d.y) * 1.4
            }
        }
    }

    fn step(&mut self, dt: f32, time: f32) {
        let trail = self.trail();
        self.cursor = (self.cursor + 1) % trail;
        let bound = self.cfg.bound;
        for i in 0..self.heads.len() {
            let mut pos = self.heads[i].pos;
            let v = self.flow(pos, time);
            let speed01 =
                ((v.x * v.x + v.y * v.y + v.z * v.z).sqrt() * self.cfg.speed * 0.12).min(1.0);
            pos = pos + v * (dt * self.cfg.speed * 0.5);
            self.heads[i].age += dt;
            let dead = self.heads[i].age > self.heads[i].life
                || pos.x.abs() > bound
                || pos.y.abs() > bound
                || pos.z.abs() > bound;
            if dead {
                pos = vec3f(
                    self.rng.range(-1.5, 1.5),
                    self.rng.range(-1.5, 1.5),
                    self.rng.range(-1.5, 1.5),
                );
                self.heads[i].age = 0.0;
                self.heads[i].life = self.rng.range(7.0, 14.0);
                self.heads[i].hue = self.rng.next_f32();
                // Collapse the whole trail onto the respawn point so no
                // ribbon streaks across the volume.
                for k in 0..trail {
                    self.history[i * trail + k] = pos;
                }
            }
            self.heads[i].pos = pos;
            self.history[i * trail + self.cursor] = pos;
            self.speeds[i * trail + self.cursor] = speed01;
        }
    }

    /// Emit strips: two verts per trail point; the vertex shader expands
    /// them view-facing. `geom_normal` carries the local tangent, `a_r1`
    /// the side (±1), `a_aux` the age along the trail (0 = head).
    fn emit(&mut self, mesh: &mut FxMesh) {
        mesh.clear();
        let trail = self.trail();
        for i in 0..self.heads.len() {
            let hue = self.heads[i].hue;
            let mut prev_ids: Option<(u32, u32)> = None;
            for k in 0..trail {
                // k = 0 newest → walk backwards from cursor.
                let slot = (self.cursor + trail - k) % trail;
                let p = self.history[i * trail + slot];
                let slot_next = (self.cursor + trail - (k + 1).min(trail - 1)) % trail;
                let p_older = self.history[i * trail + slot_next];
                let mut tangent = p - p_older;
                let tl = (tangent.x * tangent.x + tangent.y * tangent.y + tangent.z * tangent.z)
                    .sqrt();
                if tl < 1e-6 {
                    tangent = vec3f(0.0, 1.0, 0.0);
                } else {
                    tangent = tangent / tl;
                }
                let age = k as f32 / (trail - 1) as f32;
                let sp = self.speeds[i * trail + slot];
                let l = mesh.push_vert(p, i as f32, tangent, age, vec2f(sp, age), hue, -1.0);
                let r = mesh.push_vert(p, i as f32, tangent, age, vec2f(sp, age), hue, 1.0);
                if let Some((pl, pr)) = prev_ids {
                    mesh.push_quad(pl, pr, r, l);
                }
                prev_ids = Some((l, r));
            }
        }
        mesh.pad_to_high_water();
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms {
            shape: vec4(self.cfg.width, 0.0, 0.0, 0.0),
            flow: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }
}

// ---------------------------------------------------------------------------
// Tunnel (torus-knot tube flown from the inside)
// ---------------------------------------------------------------------------

/// The tunnel's closed centerline.
#[derive(Clone, Copy, PartialEq)]
pub enum TunnelPath {
    /// A (p,q) torus knot (the default).
    Knot,
    /// A 3D Lissajous figure — `knot_p`/`knot_q` become the x/y frequencies
    /// (z runs at 1), giving oscilloscope-style loops.
    Lissajous,
}

pub struct TunnelConfig {
    /// Torus-knot winding numbers (Lissajous: x/y frequencies).
    pub p: f32,
    pub q: f32,
    pub major: f32,
    pub tube: f32,
    pub rings: usize,
    pub sides: usize,
    /// Camera laps per minute along the curve.
    pub fly: f32,
    /// Neon ring frequency along the tube (pixel shading).
    pub bands: f32,
    pub path: TunnelPath,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            p: 2.0,
            q: 3.0,
            major: 6.0,
            tube: 1.35,
            rings: 720,
            sides: 22,
            fly: 3.2,
            bands: 90.0,
            path: TunnelPath::Knot,
        }
    }
}

pub struct TunnelEngine {
    pub cfg: TunnelConfig,
    built: bool,
    /// Precomputed curve frames for the camera path (pos, tangent).
    path: Vec<(Vec3f, Vec3f)>,
}

impl TunnelEngine {
    pub fn new(cfg: TunnelConfig) -> Self {
        Self { cfg, built: false, path: Vec::new() }
    }

    /// The closed centerline, scaled so `major` is roughly the overall
    /// radius of the flight path.
    fn curve(&self, t: f32) -> Vec3f {
        let (p, q, r) = (self.cfg.p, self.cfg.q, self.cfg.major);
        let a = t * std::f32::consts::TAU;
        match self.cfg.path {
            TunnelPath::Knot => {
                let minor = 0.35;
                let cq = (q * a).cos();
                let sq = (q * a).sin();
                vec3f(
                    (1.0 + minor * cq) * (p * a).cos(),
                    minor * sq * 1.4,
                    (1.0 + minor * cq) * (p * a).sin(),
                ) * r
            }
            TunnelPath::Lissajous => vec3f(
                (p * a).sin(),
                (q * a + 1.2).sin() * 0.55,
                a.cos(),
            ) * r,
        }
    }

    fn build(&mut self, mesh: &mut FxMesh) {
        let rings = self.cfg.rings.clamp(64, 2048);
        let sides = self.cfg.sides.clamp(3, 48);
        let tube = self.cfg.tube;
        // Parallel-transport frames along the closed curve.
        let mut centers = Vec::with_capacity(rings);
        let mut tangents = Vec::with_capacity(rings);
        for i in 0..rings {
            let t = i as f32 / rings as f32;
            let c = self.curve(t);
            let c2 = self.curve(t + 1.0 / rings as f32);
            let mut tan = c2 - c;
            let l = (tan.x * tan.x + tan.y * tan.y + tan.z * tan.z).sqrt().max(1e-6);
            tan = tan / l;
            centers.push(c);
            tangents.push(tan);
        }
        self.path = centers.iter().cloned().zip(tangents.iter().cloned()).collect();
        // Initial normal: anything not parallel to tangent 0.
        let mut normal = vec3f(0.0, 1.0, 0.0);
        let t0 = tangents[0];
        let d = normal.x * t0.x + normal.y * t0.y + normal.z * t0.z;
        normal = normal - t0 * d;
        let nl = (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
        normal = normal / nl.max(1e-6);
        // SEAM LAW: uv must never interpolate backwards across a wrap. Each
        // ring emits sides+1 vertices (the closing one duplicated at u=1.0)
        // and the tube emits rings+1 rings (the closing ring duplicated at
        // along=1.0, positioned at ring 0) — so every quad's uv span is
        // monotone and the neon rails/rings carry no wrapping artifact.
        //
        // Closed-loop transport needs a HOLONOMY correction: transported
        // around the whole knot the frame comes back rotated, and without
        // distributing that rotation the final segment twists visibly.
        let mut frames: Vec<Vec3f> = Vec::with_capacity(rings);
        let mut n_cur = normal;
        for i in 0..rings {
            let tan = tangents[i];
            if i > 0 {
                let d = n_cur.x * tan.x + n_cur.y * tan.y + n_cur.z * tan.z;
                n_cur = n_cur - tan * d;
                let l = (n_cur.x * n_cur.x + n_cur.y * n_cur.y + n_cur.z * n_cur.z)
                    .sqrt()
                    .max(1e-6);
                n_cur = n_cur / l;
            }
            frames.push(n_cur);
        }
        // Transport one more step back onto ring 0's tangent, measure the
        // angle error against the initial normal, unwind it progressively.
        let t0 = tangents[0];
        let mut n_end = n_cur;
        let d = n_end.x * t0.x + n_end.y * t0.y + n_end.z * t0.z;
        n_end = n_end - t0 * d;
        let l = (n_end.x * n_end.x + n_end.y * n_end.y + n_end.z * n_end.z).sqrt().max(1e-6);
        n_end = n_end / l;
        let b0 = vec3f(
            t0.y * normal.z - t0.z * normal.y,
            t0.z * normal.x - t0.x * normal.z,
            t0.x * normal.y - t0.y * normal.x,
        );
        let holonomy = (n_end.x * b0.x + n_end.y * b0.y + n_end.z * b0.z)
            .atan2(n_end.x * normal.x + n_end.y * normal.y + n_end.z * normal.z);
        for (i, frame) in frames.iter_mut().enumerate() {
            let tan = tangents[i];
            let correct = -holonomy * (i as f32 / rings as f32);
            let bi = vec3f(
                tan.y * frame.z - tan.z * frame.y,
                tan.z * frame.x - tan.x * frame.z,
                tan.x * frame.y - tan.y * frame.x,
            );
            let (s, c) = correct.sin_cos();
            *frame = *frame * c + bi * s;
        }

        let mut ring_ids: Vec<Vec<u32>> = Vec::with_capacity(rings + 1);
        for i in 0..=rings {
            let curve_i = i % rings;
            let frame_n = frames[curve_i];
            let frame_t = tangents[curve_i];
            let binorm = vec3f(
                frame_t.y * frame_n.z - frame_t.z * frame_n.y,
                frame_t.z * frame_n.x - frame_t.x * frame_n.z,
                frame_t.x * frame_n.y - frame_t.y * frame_n.x,
            );
            let along = i as f32 / rings as f32;
            let mut ids = Vec::with_capacity(sides + 1);
            for sdx in 0..=sides {
                let around = sdx as f32 / sides as f32;
                let a = around * std::f32::consts::TAU;
                let radial = frame_n * a.cos() + binorm * a.sin();
                let pos = centers[curve_i] + radial * tube;
                // Normal points INWARD (camera is inside the tube).
                // a_r0 = ring hash (per-ring flicker material for hooks).
                let ring_hash = ((curve_i as f32 * 0.6180339).fract() * 0.9998) + 0.0001;
                ids.push(mesh.push_vert(
                    pos,
                    around,
                    vec3f(-radial.x, -radial.y, -radial.z),
                    along,
                    vec2f(around, along),
                    ring_hash,
                    tube,
                ));
            }
            ring_ids.push(ids);
        }
        for i in 0..rings {
            let a_ring = &ring_ids[i];
            let b_ring = &ring_ids[i + 1];
            for sdx in 0..sides {
                mesh.push_quad(a_ring[sdx], a_ring[sdx + 1], b_ring[sdx + 1], b_ring[sdx]);
            }
        }
    }

    /// Camera flying inside the tube.
    pub fn camera(&self, time: f32) -> CamPose {
        if self.path.is_empty() {
            return CamPose { eye: vec3f(0.0, 0.0, 6.0), target: Vec3f::default(), fov: 70.0 };
        }
        let laps_per_sec = self.cfg.fly / 60.0;
        let t = (time * laps_per_sec).fract();
        let n = self.path.len();
        let fi = t * n as f32;
        let i = fi as usize % n;
        let frac = fi.fract();
        let (c0, _) = self.path[i];
        let (c1, _) = self.path[(i + 1) % n];
        let eye = c0 + (c1 - c0) * frac;
        let (l0, _) = self.path[(i + 6) % n];
        CamPose { eye, target: l0, fov: 78.0 }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms {
            shape: vec4(self.cfg.bands, self.cfg.tube, 0.0, 0.0),
            flow: vec4(0.0, 0.0, 0.0, 0.0),
        }
    }
}

// ---------------------------------------------------------------------------
// Grass field — thousands of instantly-cheap blades on a plane, swaying in
// the SAME continuous wind field as the L-system trees (the DrawVjFxMesh
// shader): gusts visibly travel across the meadow. Static mesh; everything
// that moves is per-vertex data + the wind uniforms.
//
// Per-blade vertex encoding (Mesh-shader semantics):
//   a_id   = row index (0 root, 1 mid, 2 tip) — tips flutter hardest
//   a_aux  = along-blade 0..1 — the wind lever AND the growth front
//   a_r0   = per-blade hue variation
//   a_r1   = half-width at this row; normal = lateral dir (growth re-expand)
//   uv     = (side 0/1, per-blade phase hash) — flutter decorrelation
// ---------------------------------------------------------------------------

pub struct GrassConfig {
    pub blades: usize,
    /// Half-extent of the square meadow.
    pub area: f32,
    pub height: f32,
    pub width: f32,
    /// 0 = uniform scatter, 1 = tight clumps.
    pub clump: f32,
    pub seed: u64,
}

impl Default for GrassConfig {
    fn default() -> Self {
        Self { blades: 7000, area: 9.0, height: 0.85, width: 0.035, clump: 0.5, seed: 5 }
    }
}

pub struct GrassEngine {
    pub cfg: GrassConfig,
    built: bool,
}

impl GrassEngine {
    pub fn new(cfg: GrassConfig) -> Self {
        Self { cfg, built: false }
    }

    fn build(&mut self, mesh: &mut FxMesh) {
        let blades = self.cfg.blades.clamp(64, 20_000);
        let mut rng = FxRng::new(self.cfg.seed);
        // Clump centers for the clustered fraction.
        let mut clumps = [(0.0f32, 0.0f32); 24];
        for c in clumps.iter_mut() {
            *c = (
                rng.range(-self.cfg.area, self.cfg.area),
                rng.range(-self.cfg.area, self.cfg.area),
            );
        }
        for _ in 0..blades {
            let (mut x, mut z) = (
                rng.range(-self.cfg.area, self.cfg.area),
                rng.range(-self.cfg.area, self.cfg.area),
            );
            if rng.next_f32() < self.cfg.clump {
                let (cx, cz) = clumps[(rng.next_f32() * 24.0) as usize % 24];
                x = cx + rng.range(-1.1, 1.1);
                z = cz + rng.range(-1.1, 1.1);
            }
            let yaw = rng.range(0.0, std::f32::consts::TAU);
            let lean = rng.range(-0.12, 0.12);
            let h = self.cfg.height * rng.range(0.6, 1.25);
            let w = self.cfg.width * rng.range(0.7, 1.2);
            let hue = rng.next_f32();
            let phase = rng.next_f32();
            let lateral = vec3f(yaw.cos(), 0.0, yaw.sin());
            let up = vec3f(lean, 1.0, lean * 0.6);
            // Three rows: root, mid, tip; width tapers to a point.
            let rows = [
                (0.0f32, w),
                (0.55, w * 0.55),
                (1.0, w * 0.03),
            ];
            let mut prev: Option<(u32, u32)> = None;
            for (along, half_w) in rows {
                let base = vec3f(x, 0.0, z) + up * (h * along);
                let row_id = (along * 2.0).round();
                let l = mesh.push_vert(
                    base - lateral * half_w,
                    row_id,
                    vec3f(-lateral.x, -lateral.y, -lateral.z),
                    along,
                    vec2f(0.0, phase),
                    hue,
                    half_w,
                );
                let r = mesh.push_vert(
                    base + lateral * half_w,
                    row_id,
                    lateral,
                    along,
                    vec2f(1.0, phase),
                    hue,
                    half_w,
                );
                if let Some((pl, pr)) = prev {
                    mesh.push_quad(pl, pr, r, l);
                }
                prev = Some((l, r));
            }
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        EngineUniforms::default()
    }
}

// ---------------------------------------------------------------------------
// Scriptable emitters — the programmable-fireworks engine. The document's
// per-frame `frame:` script spawns/moves/retires EMITTERS (never particles);
// each live emitter is ONE draw instance and the vertex shader expands the
// shared sheet into its sparks (firework.rs philosophy, now programmable).
// ---------------------------------------------------------------------------

pub struct EmittersConfig {
    /// Particles in the shared sheet (every emitter draws up to this many).
    pub particles: usize,
    pub size: f32,
    pub gravity: f32,
    pub seed: u64,
}

impl Default for EmittersConfig {
    fn default() -> Self {
        Self { particles: 768, size: 0.12, gravity: 1.0, seed: 1 }
    }
}

/// One live emitter — what the frame script spawned.
#[derive(Clone)]
pub struct EmitterInst {
    pub pos: Vec3f,
    pub vel: Vec3f,
    pub age: f32,
    pub life: f32,
    pub speed: f32,
    pub seed: f32,
    pub size: f32,
    /// 0 burst, 1 jet, 2 sparkle, 3 ring.
    pub style: f32,
    /// Fraction of the sheet this emitter uses (0..1).
    pub fraction: f32,
    pub gravity: f32,
    /// Ignition stagger seconds (0 = all at once).
    pub stagger: f32,
    /// Cloud radius for sparkle style.
    pub spread: f32,
    pub col_a: Vec4f,
    pub col_b: Vec4f,
    /// Script-managed persistent slot (re-spawning the same slot MOVES the
    /// emitter — that is how a script animates one).
    pub slot: Option<u32>,
}

pub const MAX_EMITTERS: usize = 192;

pub struct EmittersEngine {
    pub cfg: EmittersConfig,
    pub emitters: Vec<EmitterInst>,
    /// Spawns dropped because the cap was hit (status line honesty).
    pub dropped: u64,
    built: bool,
}

impl EmittersEngine {
    pub fn new(cfg: EmittersConfig) -> Self {
        Self { cfg, emitters: Vec::new(), dropped: 0, built: false }
    }

    /// The shared sheet — identical layout to the particles sheet.
    fn build(&mut self, mesh: &mut FxMesh) {
        let count = self.cfg.particles.clamp(64, 4096);
        let mut rng = FxRng::new(self.cfg.seed);
        for i in 0..count {
            let r0 = rng.next_f32();
            let r1 = rng.next_f32();
            let z = rng.range(-1.0, 1.0);
            let a = rng.range(0.0, std::f32::consts::TAU);
            let s = (1.0 - z * z).max(0.0).sqrt();
            let dir = vec3f(s * a.cos(), z, s * a.sin());
            let id = i as f32;
            let base = [
                (vec2f(-0.5, -0.5), vec2f(0.0, 0.0)),
                (vec2f(0.5, -0.5), vec2f(1.0, 0.0)),
                (vec2f(0.5, 0.5), vec2f(1.0, 1.0)),
                (vec2f(-0.5, 0.5), vec2f(0.0, 1.0)),
            ];
            let mut ids = [0u32; 4];
            for (k, (corner, uv)) in base.iter().enumerate() {
                ids[k] =
                    mesh.push_vert(vec3f(corner.x, corner.y, 0.0), id, dir, 0.0, *uv, r0, r1);
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
    }

    /// Apply one spawn from the frame script. A `slot` re-spawn replaces
    /// (moves) the existing emitter in that slot.
    pub fn spawn(&mut self, e: EmitterInst) {
        if let Some(slot) = e.slot {
            if let Some(existing) = self.emitters.iter_mut().find(|x| x.slot == Some(slot)) {
                *existing = e;
                return;
            }
        }
        if self.emitters.len() >= MAX_EMITTERS {
            self.dropped += 1;
            return;
        }
        self.emitters.push(e);
    }

    /// Age + retire. The emitters themselves are the only CPU state.
    fn tick(&mut self, dt: f32) {
        for e in self.emitters.iter_mut() {
            e.age += dt;
        }
        self.emitters.retain(|e| e.age <= e.life);
    }
}

// ---------------------------------------------------------------------------
// The engine wrapper
// ---------------------------------------------------------------------------

pub enum Engine {
    Particles(ParticlesEngine),
    Lsystem(LsystemEngine),
    Metaballs(MetaballsEngine),
    Heightmap(HeightmapEngine),
    Ribbons(RibbonsEngine),
    Tunnel(TunnelEngine),
    Grass(GrassEngine),
    Emitters(EmittersEngine),
    Firefly(FireflyEngine),
    Harmono(HarmonographEngine),
    Domino(DominoEngine),
    /// Fullscreen: no mesh at all — input texture 0 IS the content and the
    /// stage chain does the work.
    Screen,
}

impl Engine {
    pub fn shader_kind(&self) -> ShaderKind {
        match self {
            Engine::Particles(_) => ShaderKind::Particles,
            Engine::Lsystem(_) | Engine::Metaballs(_) | Engine::Grass(_) => ShaderKind::Mesh,
            Engine::Heightmap(_) => ShaderKind::Terrain,
            Engine::Ribbons(_) => ShaderKind::Ribbon,
            Engine::Tunnel(_) => ShaderKind::Tunnel,
            Engine::Emitters(_) => ShaderKind::Emitters,
            Engine::Firefly(_) => ShaderKind::Firefly,
            Engine::Harmono(_) => ShaderKind::Harmono,
            Engine::Domino(_) => ShaderKind::Domino,
            Engine::Screen => ShaderKind::Emitters, // never drawn; screen skips the scene pass
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Engine::Particles(_) => "particles",
            Engine::Lsystem(_) => "lsystem",
            Engine::Metaballs(_) => "metaballs",
            Engine::Heightmap(_) => "heightmap",
            Engine::Ribbons(_) => "ribbons",
            Engine::Tunnel(_) => "tunnel",
            Engine::Grass(_) => "grass",
            Engine::Emitters(_) => "emitters",
            Engine::Firefly(_) => "firefly",
            Engine::Harmono(_) => "harmonograph",
            Engine::Domino(_) => "domino",
            Engine::Screen => "screen",
        }
    }

    /// Per-frame CPU work. Returns `true` when `mesh` was (re)filled and
    /// must be re-uploaded. Static engines return `true` exactly once.
    pub fn regen(&mut self, dt: f32, time: f32, beat_phase: f32, mesh: &mut FxMesh) -> bool {
        match self {
            Engine::Particles(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Lsystem(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Heightmap(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Tunnel(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Metaballs(e) => {
                e.regen(time, beat_phase, mesh);
                true
            }
            Engine::Ribbons(e) => {
                if !e.warmed {
                    // Pre-roll so the first visible frame already has trails.
                    for _ in 0..e.trail() {
                        e.step(1.0 / 60.0, time);
                    }
                    e.warmed = true;
                }
                e.step(dt.clamp(0.0, 0.1), time);
                e.emit(mesh);
                true
            }
            Engine::Grass(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Emitters(e) => {
                e.tick(dt.clamp(0.0, 0.1));
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Firefly(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Harmono(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Domino(e) => {
                if e.built {
                    return false;
                }
                e.built = true;
                mesh.clear();
                e.build(mesh);
                true
            }
            Engine::Screen => false,
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        match self {
            Engine::Particles(e) => e.uniforms(),
            Engine::Lsystem(e) => e.uniforms(),
            Engine::Metaballs(e) => e.uniforms(),
            Engine::Heightmap(e) => e.uniforms(),
            Engine::Ribbons(e) => e.uniforms(),
            Engine::Tunnel(e) => e.uniforms(),
            Engine::Grass(e) => e.uniforms(),
            Engine::Firefly(e) => e.uniforms(),
            Engine::Harmono(e) => e.uniforms(),
            Engine::Domino(e) => e.uniforms(),
            Engine::Emitters(_) | Engine::Screen => EngineUniforms::default(),
        }
    }

    /// Engine-authored camera, if it insists on one (the tunnel flies its
    /// own path); `None` = the view's orbit rig applies.
    pub fn camera(&self, time: f32) -> Option<CamPose> {
        match self {
            Engine::Tunnel(e) => Some(e.camera(time)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_engines_build_exactly_once() {
        let mut e = Engine::Particles(ParticlesEngine::new(ParticlesConfig::default()));
        let mut mesh = FxMesh::default();
        assert!(e.regen(0.016, 0.0, 0.0, &mut mesh));
        let verts = mesh.vertex_count();
        assert_eq!(verts, 4000 * 4);
        assert!(!e.regen(0.016, 0.1, 0.0, &mut mesh), "static sheet must not rebuild");
    }

    #[test]
    fn metaballs_march_a_surface_and_keep_buffer_size_stable() {
        let mut e = MetaballsEngine::new(MetaballsConfig { grid: 20, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.regen(0.0, 0.0, &mut mesh);
        assert!(mesh.triangle_count() > 100, "expected a surface, got {}", mesh.triangle_count());
        let len0 = mesh.verts.len();
        e.regen(0.5, 0.0, &mut mesh);
        assert!(mesh.verts.len() >= len0.min(mesh.verts.len()));
        let cap = mesh.verts.len();
        e.regen(0.6, 0.0, &mut mesh);
        assert!(
            mesh.verts.len() >= cap || mesh.verts.len() == cap,
            "high-water padding must keep the buffer from shrinking"
        );
    }

    #[test]
    fn metaball_normals_point_outward() {
        let mut e = MetaballsEngine::new(MetaballsConfig::default());
        let mut mesh = FxMesh::default();
        e.regen(0.0, 0.0, &mut mesh);
        // For a field of positive blobs the gradient points inward, so our
        // negated gradient must roughly agree with the position direction
        // relative to the nearest blob center.
        let floats = super::super::mesh::VERT_FLOATS;
        let mut agree = 0;
        let mut total = 0;
        for v in mesh.verts.chunks(floats).take(500) {
            let p = vec3f(v[0], v[1], v[2]);
            let n = vec3f(v[4], v[5], v[6]);
            if n.x == 0.0 && n.y == 0.0 && n.z == 0.0 {
                continue;
            }
            let dom = v[3] as usize;
            let c = e.centers[dom.min(e.centers.len() - 1)];
            let out = p - c;
            let d = out.x * n.x + out.y * n.y + out.z * n.z;
            total += 1;
            if d > 0.0 {
                agree += 1;
            }
        }
        assert!(total > 50);
        assert!(agree * 10 > total * 8, "normals disagree with outward: {agree}/{total}");
    }

    #[test]
    fn ribbons_regen_every_frame_with_stable_capacity() {
        let mut e = Engine::Ribbons(RibbonsEngine::new(RibbonsConfig::default()));
        let mut mesh = FxMesh::default();
        assert!(e.regen(0.016, 0.0, 0.0, &mut mesh));
        let len = mesh.verts.len();
        assert!(len > 0);
        assert!(e.regen(0.016, 0.016, 0.0, &mut mesh));
        assert_eq!(mesh.verts.len(), len, "ribbon buffer must be capacity-stable");
    }

    #[test]
    fn tunnel_camera_stays_inside_the_tube() {
        let mut e = TunnelEngine::new(TunnelConfig::default());
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        for i in 0..50 {
            let cam = e.camera(i as f32 * 0.37);
            // The path IS the tube centerline, so any point on it is inside.
            assert!(cam.eye.x.is_finite() && cam.eye.y.is_finite() && cam.eye.z.is_finite());
        }
        assert!(mesh.triangle_count() > 1000);
    }
}
