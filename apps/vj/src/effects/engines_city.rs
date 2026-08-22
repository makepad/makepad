//! City Flyover — a procedural night city flown over by a banking camera.
//!
//! The whole city is a STATIC mesh built once at load: tower boxes on a
//! street grid, a ground plane, a sky cylinder, and (for the TRON style)
//! light-cycle trail walls sweeping the streets. Everything that moves per
//! frame is shader math off the vertex stream: the window grid is a PIXEL
//! pattern of the facade uv (baked in window units so `floor(uv)` IS the
//! window index), per-window flicker is `hash(tower, window, floor(beat))`,
//! the trail front is a pure function of the beat, and the camera banks by
//! rotating VIEW-space xy with a roll derived from the same analytic drift
//! path the Rust camera flies (see CAMERA CONTRACT below).
//!
//! Three styles share the mesh and the shader (`u_shape.x`):
//!   0 night — lit windows, rooftop beacons, starfield sky
//!   1 retro — wireframe-neon facades, sun-stripe horizon, grid streets
//!   2 tron  — dark fine-grid floor, edge-glow towers, beat-swept trails
//!
//! # CAMERA CONTRACT (Rust `camera()` and the shader roll MUST agree)
//! `tc = time * fly * 0.05`; drift `x = sin(tc)`, `z = sin(0.73*tc + 1.7)`
//! (scaled by `0.55 * extent` in Rust). The shader recomputes the heading
//! rate of that exact path and rolls view space by `bank * dθ/dt`.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   a_id  = tower id / trail id
//!   a_aux = CLASS: 0 tower, 1 ground, 2 sky, 3 trail wall
//!   uv    = tower: facade uv in WINDOW units; ground: world xz;
//!           sky: (azimuth01, height01); trail: (arc01, height01)
//!   a_r0  = tower hash / trail hue01
//!   a_r1  = tower height (world) / trail phase01
//!   normal = face normal (sky: inward, trail: lateral)
//!
//! # Document keys (`engine: "city"`)
//! `style` ("night" | "retro" | "tron"), `blocks` (8, ≤14 blocks/side),
//! `block` (6.0 block pitch), `street` (0.34 street fraction of the pitch),
//! `towers` (240 cap), `max_h` (10), `win` (0.30 window size, world),
//! `density` (0.55 lit fraction), `flicker` (0.12 fraction re-rolled per
//! beat), `trails` (0, ≤16), `trail_beats` (8 sweep loop), `wall_h` (1.3),
//! `alt` (camera altitude, default 1.05 * max_h), `fly` (1.0 speed),
//! `bank` (1.0 roll gain). Bindings: `p0` ADDS window density (the city
//! wakes up with energy), `p1` = sun/trail-head gain, `p2` = beacon/edge
//! glow gain. Hook: `fx_color` (facade base tint; t = height01).
//!
//! Content coupling (`content:` → `fog.z`): night facades play the channel
//! video as chunky window-pixel billboards (per-tower crop, flicker still
//! gates); retro/tron tint the neon wire, street grid and trail walls with
//! the video instead.

use super::engines::{CamPose, EngineUniforms};
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

#[derive(Clone, Copy, PartialEq)]
pub enum CityStyle {
    Night = 0,
    Retro = 1,
    Tron = 2,
}

impl CityStyle {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "night" | "realistic" => Self::Night,
            "retro" | "synthwave" => Self::Retro,
            "tron" | "arena" => Self::Tron,
            _ => return None,
        })
    }
}

pub struct CityConfig {
    pub style: CityStyle,
    /// City blocks per side.
    pub blocks: usize,
    /// Block pitch (world units, street included).
    pub block: f32,
    /// Street width as a fraction of the pitch.
    pub street: f32,
    /// Hard cap on towers placed.
    pub towers: usize,
    /// Tallest downtown tower (world units).
    pub max_h: f32,
    /// Window cell size (world units) — facade uv is baked in these units.
    pub win: f32,
    /// Lit-window fraction 0..1.
    pub density: f32,
    /// Fraction of windows that re-roll their state each beat.
    pub flicker: f32,
    /// Light-cycle trail walls (0 = none; the TRON ingredient).
    pub trails: usize,
    /// Beats per full trail sweep.
    pub trail_beats: f32,
    /// Trail wall height.
    pub wall_h: f32,
    /// Camera altitude (<= 0 = auto: 1.05 * max_h).
    pub alt: f32,
    /// Camera speed multiplier.
    pub fly: f32,
    /// Camera bank (roll) gain.
    pub bank: f32,
    pub seed: u64,
}

impl Default for CityConfig {
    fn default() -> Self {
        Self {
            style: CityStyle::Night,
            blocks: 8,
            block: 6.0,
            street: 0.34,
            towers: 240,
            max_h: 10.0,
            win: 0.30,
            density: 0.55,
            flicker: 0.12,
            trails: 0,
            trail_beats: 8.0,
            wall_h: 1.3,
            alt: -1.0,
            fly: 1.0,
            bank: 1.0,
            seed: 9,
        }
    }
}

pub struct CityEngine {
    pub cfg: CityConfig,
    pub(crate) built: bool,
    /// Half-extent of the city footprint (set at build).
    pub extent: f32,
    pub placed: usize,
}

impl CityEngine {
    pub fn new(cfg: CityConfig) -> Self {
        let blocks = cfg.blocks.clamp(2, 14) as f32;
        let extent = blocks * Self::san(cfg.block, 6.0).clamp(1.0, 40.0) * 0.5;
        Self { cfg, built: false, extent, placed: 0 }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    /// One tower: 4 facades + roof (no bottom), facade uv in window units.
    #[allow(clippy::too_many_arguments)]
    fn emit_tower(
        mesh: &mut FxMesh,
        id: f32,
        hash: f32,
        cx: f32,
        cz: f32,
        hx: f32,
        hz: f32,
        h: f32,
        win: f32,
    ) {
        // Whole window counts so no cell is cut at an edge.
        let wu_x = (hx * 2.0 / win).round().max(1.0);
        let wu_z = (hz * 2.0 / win).round().max(1.0);
        let floors = (h / (win * 1.4)).round().max(1.0);
        // (normal, corners a..d, u window count) — corners CCW seen from
        // outside; v runs up the facade.
        let faces: [([f32; 3], [[f32; 3]; 4], f32); 4] = [
            // +z
            ([0.0, 0.0, 1.0],
             [[-hx, 0.0, hz], [hx, 0.0, hz], [hx, h, hz], [-hx, h, hz]], wu_x),
            // -z
            ([0.0, 0.0, -1.0],
             [[hx, 0.0, -hz], [-hx, 0.0, -hz], [-hx, h, -hz], [hx, h, -hz]], wu_x),
            // +x
            ([1.0, 0.0, 0.0],
             [[hx, 0.0, hz], [hx, 0.0, -hz], [hx, h, -hz], [hx, h, hz]], wu_z),
            // -x
            ([-1.0, 0.0, 0.0],
             [[-hx, 0.0, -hz], [-hx, 0.0, hz], [-hx, h, hz], [-hx, h, -hz]], wu_z),
        ];
        for (n, corners, wu) in &faces {
            let normal = vec3f(n[0], n[1], n[2]);
            let uvs = [
                vec2f(0.0, 0.0),
                vec2f(*wu, 0.0),
                vec2f(*wu, floors),
                vec2f(0.0, floors),
            ];
            let mut ids = [0u32; 4];
            for (k, c) in corners.iter().enumerate() {
                ids[k] = mesh.push_vert(
                    vec3f(cx + c[0], c[1], cz + c[2]),
                    id,
                    normal,
                    0.0,
                    uvs[k],
                    hash,
                    h,
                );
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
        // Roof: uv 0..1 (beacon pattern space).
        let up = vec3f(0.0, 1.0, 0.0);
        let rc = [
            ([-hx, h, hz], vec2f(0.0, 0.0)),
            ([hx, h, hz], vec2f(1.0, 0.0)),
            ([hx, h, -hz], vec2f(1.0, 1.0)),
            ([-hx, h, -hz], vec2f(0.0, 1.0)),
        ];
        let mut ids = [0u32; 4];
        for (k, (c, uv)) in rc.iter().enumerate() {
            ids[k] = mesh.push_vert(
                vec3f(cx + c[0], c[1], cz + c[2]),
                id,
                up,
                0.0,
                *uv,
                hash,
                h,
            );
        }
        mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
    }

    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let mut rng = FxRng::new(self.cfg.seed);
        let blocks = self.cfg.blocks.clamp(2, 14);
        let pitch = Self::san(self.cfg.block, 6.0).clamp(1.0, 40.0);
        let street = Self::san(self.cfg.street, 0.34).clamp(0.05, 0.8);
        let max_h = Self::san(self.cfg.max_h, 10.0).clamp(1.0, 60.0);
        let win = Self::san(self.cfg.win, 0.30).clamp(0.05, 2.0);
        let extent = blocks as f32 * pitch * 0.5;
        self.extent = extent;

        // ---- ground plane (class 1; uv = world xz for the street grid) ----
        // Reaches past the sky cylinder (2.6 * extent) so the fogged ground
        // MEETS the sky — a shorter plane shows its own edge as a dark band
        // under the horizon.
        let g = extent * 2.7;
        let up = vec3f(0.0, 1.0, 0.0);
        let g0 = mesh.push_vert(vec3f(-g, 0.0, -g), 0.0, up, 1.0, vec2f(-g, -g), 0.0, 0.0);
        let g1 = mesh.push_vert(vec3f(g, 0.0, -g), 0.0, up, 1.0, vec2f(g, -g), 0.0, 0.0);
        let g2 = mesh.push_vert(vec3f(g, 0.0, g), 0.0, up, 1.0, vec2f(g, g), 0.0, 0.0);
        let g3 = mesh.push_vert(vec3f(-g, 0.0, g), 0.0, up, 1.0, vec2f(-g, g), 0.0, 0.0);
        mesh.push_quad(g0, g1, g2, g3);

        // ---- sky cylinder (class 2; inward normals; never fogged) ---------
        let sky_r = extent * 2.6;
        let sky_top = (extent * 1.1).max(max_h * 2.6);
        let segs = 48usize;
        let mut ring_lo = Vec::with_capacity(segs + 1);
        let mut ring_hi = Vec::with_capacity(segs + 1);
        for i in 0..=segs {
            let az = i as f32 / segs as f32;
            let a = az * std::f32::consts::TAU;
            let (s, c) = a.sin_cos();
            let n = vec3f(-s, 0.0, -c);
            ring_lo.push(mesh.push_vert(
                vec3f(s * sky_r, -0.5, c * sky_r), 0.0, n, 2.0, vec2f(az, 0.0), 0.0, 0.0,
            ));
            ring_hi.push(mesh.push_vert(
                vec3f(s * sky_r, sky_top, c * sky_r), 0.0, n, 2.0, vec2f(az, 1.0), 0.0, 0.0,
            ));
        }
        for i in 0..segs {
            mesh.push_quad(ring_lo[i], ring_lo[i + 1], ring_hi[i + 1], ring_hi[i]);
        }

        // ---- towers (class 0): 2x2 pads per block, downtown height profile
        let cap = self.cfg.towers.clamp(4, 900);
        let mut placed = 0usize;
        let mut id = 0.0f32;
        'blocks: for bx in 0..blocks {
            for bz in 0..blocks {
                let bcx = (bx as f32 + 0.5) * pitch - extent;
                let bcz = (bz as f32 + 0.5) * pitch - extent;
                let inner = pitch * (1.0 - street) * 0.5; // buildable half-extent
                let d01 = ((bcx * bcx + bcz * bcz).sqrt() / extent).min(1.0);
                let profile = 1.0 - 0.78 * d01.powf(1.5);
                for (px, pz) in [(-0.5f32, -0.5f32), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)] {
                    if rng.next_f32() < 0.18 {
                        continue; // park / parking-lot gap
                    }
                    let hx = inner * rng.range(0.30, 0.46);
                    let hz = inner * rng.range(0.30, 0.46);
                    let cx = bcx + px * inner - px.signum() * (inner * 0.5 - hx).max(0.0) * rng.next_f32();
                    let cz = bcz + pz * inner - pz.signum() * (inner * 0.5 - hz).max(0.0) * rng.next_f32();
                    let mut h = max_h * profile * rng.range(0.28, 1.0);
                    // Occasional spire downtown.
                    if d01 < 0.4 && rng.next_f32() < 0.10 {
                        h = max_h * rng.range(0.95, 1.15);
                    }
                    let h = h.max(win * 2.0);
                    let hash = rng.next_f32();
                    Self::emit_tower(mesh, id, hash, cx, cz, hx, hz, h, win);
                    id += 1.0;
                    placed += 1;
                    if placed >= cap {
                        break 'blocks;
                    }
                }
            }
        }
        self.placed = placed;

        // ---- trail walls (class 3): turtle walks on the street lattice ----
        let trails = self.cfg.trails.min(16);
        let wall_h = Self::san(self.cfg.wall_h, 1.3).clamp(0.1, 8.0);
        for t in 0..trails {
            let hue = (t as f32 + 0.5) / trails.max(1) as f32;
            let phase = rng.next_f32();
            // Start on a street crossing (block corner lattice).
            let lat = |v: i32| v as f32 * pitch - extent;
            let mut gx = rng.range(1.0, blocks as f32 - 1.0) as i32;
            let mut gz = rng.range(1.0, blocks as f32 - 1.0) as i32;
            let mut dir = match (rng.next_f32() * 4.0) as u32 % 4 {
                0 => (1i32, 0i32),
                1 => (-1, 0),
                2 => (0, 1),
                _ => (0, -1),
            };
            // Total path length in lattice steps (arc normalizer).
            let steps = 14usize;
            let mut pts = Vec::with_capacity(steps + 1);
            pts.push((gx, gz));
            for _ in 0..steps {
                // Turn ~40% of the time; clamp to the interior lattice.
                if rng.next_f32() < 0.4 {
                    dir = if rng.next_f32() < 0.5 { (dir.1, dir.0) } else { (-dir.1, -dir.0) };
                }
                let nx = (gx + dir.0).clamp(0, blocks as i32);
                let nz = (gz + dir.1).clamp(0, blocks as i32);
                if nx == gx && nz == gz {
                    dir = (-dir.0, -dir.1);
                    continue;
                }
                gx = nx;
                gz = nz;
                pts.push((gx, gz));
            }
            let total = (pts.len() - 1).max(1) as f32;
            for k in 0..pts.len() - 1 {
                let (ax, az) = pts[k];
                let (bx2, bz2) = pts[k + 1];
                let a = vec3f(lat(ax), 0.02, lat(az));
                let b = vec3f(lat(bx2), 0.02, lat(bz2));
                let seg = b - a;
                let sl = (seg.x * seg.x + seg.z * seg.z).sqrt().max(1e-4);
                let lateral = vec3f(-seg.z / sl, 0.0, seg.x / sl);
                let arc0 = k as f32 / total;
                let arc1 = (k + 1) as f32 / total;
                let tid = t as f32;
                let v = [
                    (a, arc0, 0.0),
                    (b, arc1, 0.0),
                    (b, arc1, 1.0),
                    (a, arc0, 1.0),
                ];
                let mut ids = [0u32; 4];
                for (i, (p, arc, h01)) in v.iter().enumerate() {
                    ids[i] = mesh.push_vert(
                        vec3f(p.x, p.y + wall_h * h01, p.z),
                        tid,
                        lateral,
                        3.0,
                        vec2f(*arc, *h01),
                        hue,
                        phase,
                    );
                }
                mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
            }
        }
    }

    /// The banking flyover. MUST match the shader's roll math — see the
    /// CAMERA CONTRACT in the module docs.
    pub fn camera(&self, time: f32) -> CamPose {
        let fly = Self::san(self.cfg.fly, 1.0).clamp(0.05, 9.9);
        let tc = time * fly * 0.05;
        let r = self.extent * 0.55;
        let max_h = Self::san(self.cfg.max_h, 10.0).clamp(1.0, 60.0);
        let alt = if self.cfg.alt > 0.0 && self.cfg.alt.is_finite() {
            self.cfg.alt
        } else {
            max_h * 1.05
        };
        let path = |t: f32| vec3f((t).sin() * r, 0.0, (0.73 * t + 1.7).sin() * r);
        let p = path(tc);
        let mut look = path(tc + 0.35);
        // Degenerate-guard: near the Lissajous turning points the look-ahead
        // can collapse onto the eye — push it further along the path.
        let d = look - p;
        if (d.x * d.x + d.z * d.z).sqrt() < r * 0.05 {
            look = path(tc + 1.1);
        }
        // Aim a FIXED horizontal distance along the heading, at a rooftop
        // height: a gentle ~17-degree descent that keeps streets AND the
        // fogged horizon/sky in frame (a steep look-down reads as a map).
        let mut dir = look - p;
        let dl = (dir.x * dir.x + dir.z * dir.z).sqrt().max(1e-4);
        dir = vec3f(dir.x / dl, 0.0, dir.z / dl);
        let ahead = self.extent * 0.85;
        let eye = vec3f(p.x, alt * (1.0 + 0.06 * (tc * 1.7).sin()), p.z);
        CamPose {
            eye,
            target: vec3f(p.x + dir.x * ahead, alt * 0.45, p.z + dir.z * ahead),
            fov: 62.0,
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        EngineUniforms {
            shape: vec4(
                self.cfg.style as i32 as f32,
                san(self.cfg.block, 6.0).clamp(1.0, 40.0),
                san(self.cfg.street, 0.34).clamp(0.05, 0.8),
                san(self.cfg.fly, 1.0).clamp(0.05, 9.9),
            ),
            flow: vec4(
                san(self.cfg.density, 0.55).clamp(0.0, 1.0),
                san(self.cfg.flicker, 0.12).clamp(0.0, 1.0),
                san(self.cfg.trail_beats, 8.0).clamp(1.0, 64.0),
                san(self.cfg.bank, 1.0).clamp(0.0, 4.0),
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // City Flyover: solid depth-tested classes (tower / ground / sky /
    // trail). The vertex applies the CAMERA-CONTRACT roll to view space and
    // collapses un-swept trail walls; the pixel shader runs the window
    // grid, street grid, sky and trail looks per class, blended per style.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxCity = set_type_default() do #(DrawVjFxCity::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        has_content: uniform(0.0)
        backface_culling: false
        alpha_blend: false
        depth_write: true

        // (class, tower id, tower hash, tower h / trail phase)
        v_attr: varying(vec4f)
        // facade uv in window units / world xz / sky uv / trail (arc, h01)
        v_uv: varying(vec2f)
        v_normal: varying(vec3f)
        v_world: varying(vec3f)
        // (base tint rgb, trail front01)
        v_base: varying(vec4f)

        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        // Document hook: facade base tint. t = height01 up the tower,
        // attr = (class, id, hash, height).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let v = 0.8 + 0.4 * attr.z
            return self.col_a.mix(self.col_b, clamp(t * 0.8 + attr.z * 0.3, 0.0, 1.0)) * v
        }

        vertex: fn() {
            let class = self.geom.geom_pad
            let idv = self.geom.geom_id
            let r0 = self.geom.geom_tail_pad_0
            let r1 = self.geom.geom_tail_pad_1
            let mut pos = self.geom.geom_pos
            // Trail walls: collapse ahead of the sweeping front. Front loops
            // once per trail_beats with a per-trail phase; p1 nudges it.
            let mut front = 0.0
            if class > 2.5 {
                let tb = max(self.flow.z, 1.0)
                front = fract((self.time_beat.y + self.user.y) / tb + r1)
                let vis = step(self.geom.geom_uv.x, front)
                pos = vec3(pos.x, 0.02 + (pos.y - 0.02) * vis, pos.z)
            }
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_world = world.xyz
            self.v_uv = self.geom.geom_uv
            self.v_normal = self.geom.geom_normal
            self.v_attr = vec4(class, idv, r0, r1)
            let h01 = clamp(pos.y / max(r1, 0.001), 0.0, 1.0)
            let base = self.fx_color(h01, vec4(class, idv, r0, r1), self.geom.geom_normal, world.xyz)
            self.v_base = vec4(base.xyz, front)
            let mut view_pos = self.draw_pass.camera_view * world
            // CAMERA CONTRACT roll: heading rate of the analytic drift path
            // (x = sin(tc), z = sin(0.73 tc + 1.7)), rolled in VIEW space so
            // the bank is a true camera roll (no aspect skew).
            let tc = self.time_beat.x * self.shape.w * 0.05
            let xd = cos(tc)
            let zd = 0.73 * cos(0.73 * tc + 1.7)
            let xdd = 0.0 - sin(tc)
            let zdd = 0.0 - 0.5329 * sin(0.73 * tc + 1.7)
            let roll = clamp(
                self.flow.w * 1.6 * (xdd * zd - zdd * xd) / max(xd * xd + zd * zd, 0.05),
                -0.5, 0.5
            )
            let cr = cos(roll)
            let sr = sin(roll)
            view_pos = vec4(
                view_pos.x * cr - view_pos.y * sr,
                view_pos.x * sr + view_pos.y * cr,
                view_pos.z,
                view_pos.w
            )
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            let style = self.shape.x
            let class = self.v_attr.x
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let dist = length(self.v_world - cam.xyz / max(cam.w, 0.0001))
            let fogf = exp(0.0 - dist * self.fog.x)
            let mut rgb = vec3(0.0, 0.0, 0.0)
            let mut fog_on = 1.0
            if class > 2.5 {
                // TRAIL WALL: solid light ribbon, hot at the head, top edge
                // brightest, aging tail dims toward the loop wrap.
                let hue = self.v_attr.z
                // CONTENT: the wall carries the channel video unrolled
                // along its arc (three copies, drifting slowly).
                let ttx = self.tex0.sample_as_bgra(
                    vec2(fract(self.v_uv.x * 1.5 + self.time_beat.x * 0.03), 1.0 - self.v_uv.y)
                )
                let wall = self.col_a.mix(self.col_b, hue).xyz
                    .mix(ttx.xyz * 1.5, clamp(self.fog.z * 1.2, 0.0, 1.0))
                let behind = max(self.v_base.w - self.v_uv.x, 0.0)
                let head = exp(0.0 - behind * 22.0) * (1.5 + self.user.y)
                let age = clamp(1.0 - behind * 1.1, 0.35, 1.0)
                let top = 0.55 + 0.45 * smoothstep(0.55, 1.0, self.v_uv.y)
                rgb = wall * (age * top * (1.1 + self.time_beat.w * 0.5))
                    + self.col_c.xyz * head
            } else { if class > 1.5 {
                // SKY: per style — stars / sun stripes / horizon glow.
                fog_on = 0.0
                let az = self.v_uv.x
                let h01 = self.v_uv.y
                if style < 0.5 {
                    let horizon = exp(0.0 - h01 * 3.5)
                    let sp = vec2(az * 340.0, h01 * 70.0)
                    let cell = floor(sp)
                    let gate = step(0.994, self.hash1(cell.x * 7.7 + cell.y * 13.3))
                    // A POINT inside the lit cell (a bare gate lights the
                    // whole rectangular cell — dashes, not stars).
                    let fc = fract(sp)
                    let dot2 = 1.0 - smoothstep(0.06, 0.22, length(fc - vec2(0.5, 0.5)))
                    let star = gate * dot2 * (0.4 + 0.6 * self.hash1(cell.x + cell.y * 3.1))
                    rgb = self.col_bg.xyz * (0.6 + horizon * 1.2)
                        + self.col_b.xyz * horizon * 0.12
                        + vec3(star, star, star) * (1.0 - horizon * 0.5)
                } else { if style < 1.5 {
                    // Synthwave sun: banded disc over a horizon gradient.
                    let daz = (fract(az - 0.25 + 0.5) - 0.5) * 2.0
                    let sd = length(vec2(daz * 7.0, (h01 - 0.26) * 3.2))
                    let disc = 1.0 - smoothstep(0.42, 0.46, sd)
                    let stripe = step(0.30 + 0.45 * smoothstep(0.30, 0.06, h01),
                        fract(h01 * 26.0))
                    let sun = disc * stripe
                    let glow = exp(0.0 - sd * 2.4) * (0.5 + self.user.y * 0.5)
                    let grad = self.col_bg.mix(self.col_b, exp(0.0 - h01 * 2.6) * 0.85)
                    let sun_col = self.col_c.mix(self.col_b, clamp(h01 * 3.0, 0.0, 1.0))
                    rgb = grad.xyz + sun_col.xyz * (sun * (1.2 + self.time_beat.w * 0.6))
                        + self.col_b.xyz * glow * 0.5
                } else {
                    rgb = self.col_bg.xyz * 0.7
                        + self.col_a.xyz * exp(0.0 - h01 * 6.0) * 0.22
                } }
            } else { if class > 0.5 {
                // GROUND: street grid from world xz; block interiors dark.
                let s = max(self.shape.y, 0.5)
                let q = fract(self.v_uv / s)
                let dm = min(min(q.x, 1.0 - q.x), min(q.y, 1.0 - q.y))
                let half = self.shape.z * 0.5
                let street = 1.0 - smoothstep(half, half + 0.03, dm)
                let lane = 1.0 - smoothstep(0.006, 0.02, dm)
                if style < 0.5 {
                    rgb = self.col_bg.xyz * 0.9
                        + self.col_a.xyz * street * (0.05 + self.time_beat.w * 0.02)
                        + self.col_c.xyz * lane * 0.10
                } else {
                    // CONTENT (retro/tron): the street plane PLAYS the clip
                    // — one copy every 8 blocks, lighting the grid and
                    // washing the asphalt between the lines, so the ground
                    // reads as a screen the city stands on.
                    let gtx = self.tex0.sample_as_bgra(fract(self.v_uv / (s * 8.0)))
                    let gc = clamp(self.fog.z * 1.2, 0.0, 1.0)
                    let ga = self.col_a.xyz.mix(gtx.xyz * 1.7, gc)
                    let wash = gtx.xyz * (self.fog.z * 0.35)
                    if style < 1.5 {
                        let edge = 1.0 - smoothstep(half * 0.9, half + 0.05, dm)
                        rgb = self.col_bg.xyz * 0.5 + wash
                            + ga * edge * (0.7 + self.time_beat.w * 0.9)
                    } else {
                        let f = fract(self.v_uv / (s * 0.25))
                        let fm = min(min(f.x, 1.0 - f.x), min(f.y, 1.0 - f.y))
                        let fine = 1.0 - smoothstep(0.01, 0.05, fm)
                        rgb = self.col_bg.xyz * 0.4 + wash
                            + ga * (fine * 0.20 + lane * (0.9 + self.time_beat.w * 0.8))
                    }
                }
            } else {
                // TOWER: roofs vs facades split on the face normal.
                let wu = self.v_uv
                let cell = floor(wu)
                let f = fract(wu)
                let tid = self.v_attr.y
                let hh = self.v_attr.w
                if self.v_normal.y > 0.5 {
                    // Roof: dark slab + a beat-blinking beacon on tall towers.
                    let bd = length(f - vec2(0.5, 0.5))
                    let blink = step(0.5, fract(self.time_beat.y * 0.5))
                    let tall = step(6.0, hh)
                    let beacon = (1.0 - smoothstep(0.05, 0.12, bd)) * blink * tall
                        * (1.0 + self.user.z)
                    rgb = self.v_base.xyz * 0.10
                        + vec3(1.0, 0.2, 0.15) * beacon * 1.6
                } else {
                    // Facade: the window grid. State = hash(tower, window)
                    // stably, RE-ROLLED each beat for a `flicker` fraction —
                    // hash(tower, window, floor(beat)) is the beat index law.
                    let wsel = self.hash1(tid * 17.31 + cell.x * 7.7 + cell.y * 13.9)
                    let re = self.hash1(
                        tid * 3.17 + cell.x * 11.3 + cell.y * 5.9
                        + floor(self.time_beat.y) * 7.77
                    )
                    let state = mix(wsel, re, step(1.0 - self.flow.y, wsel))
                    let dens = clamp(self.flow.x + self.user.x, 0.0, 1.0)
                    let lit = step(state, dens)
                    let inwin = step(0.18, f.x) * step(f.x, 0.82)
                        * step(0.22, f.y) * step(f.y, 0.78)
                    let warm = self.hash1(tid * 5.3 + cell.x * 3.7 + cell.y * 9.1)
                    let wcol = vec3(1.0, 0.85, 0.6).mix(vec3(0.75, 0.88, 1.0), step(0.6, warm))
                    let wb = 0.55 + 0.45 * self.hash1(cell.x * 1.7 + cell.y * 2.9 + tid)
                    // CONTENT: facades become VIDEO BILLBOARDS — every
                    // window is one chunky pixel of the channel video and
                    // each tower runs the clip from its own bottom-left
                    // corner, so a whole skyline of screens plays it at
                    // once. (The first pass gave every tower a RANDOM crop
                    // offset: correct as "a city of screens", unreadable as
                    // a picture — the frames never lined up with each
                    // other.) A dim wash carries the clip across the dark
                    // glass between windows so the picture is continuous,
                    // while grid, density and beat flicker stay in charge.
                    let span = vec2(10.0, 14.0)
                    let fuv = (cell + vec2(0.5, 0.5)) / span
                    let ftx = self.tex0.sample_as_bgra(vec2(fract(fuv.x), fract(0.0 - fuv.y)))
                    // A SECOND, continuous sample for the glass between the
                    // windows: the chunky per-window mosaic is the look, but
                    // a 10x14 pixel grid alone is too coarse to recognise —
                    // the smooth wash underneath carries the actual picture.
                    let suv = wu / span
                    let stx = self.tex0.sample_as_bgra(vec2(fract(suv.x), fract(0.0 - suv.y)))
                    let cm = self.fog.z
                    if style < 0.5 {
                        let win = wcol * (wb * lit)
                        let vid = ftx.xyz * (0.30 + 1.05 * lit * wb)
                        rgb = self.v_base.xyz * 0.045
                            + stx.xyz * (cm * 0.55)
                            + win.mix(vid, clamp(cm * 1.2, 0.0, 1.0))
                                * (inwin * (1.0 + self.time_beat.w * 0.25))
                    } else {
                        // Retro/tron: neon wire grid on dark glass; lit
                        // windows become scanline accents. Content tints
                        // the wire per window cell — a mosaic of the video.
                        let gd = min(min(f.x, 1.0 - f.x), min(f.y, 1.0 - f.y))
                        let wire = 1.0 - smoothstep(0.02, 0.10, gd)
                        let gain = 0.5 + self.time_beat.w * 0.8 + self.user.z * 0.5
                        let neon = self.v_base.xyz.mix(ftx.xyz * 1.7, clamp(cm * 1.15, 0.0, 1.0))
                            * wire * gain
                        // The glass between the wires carries the clip too:
                        // a wire mosaic alone is too little surface to read.
                        let glass = stx.xyz * (cm * 0.55)
                        if style < 1.5 {
                            rgb = self.col_bg.xyz * 0.25 + neon + glass
                                + wcol * lit * inwin * 0.12
                        } else {
                            let ed = min(f.y, 1.0 - f.y)
                            let floor_line = 1.0 - smoothstep(0.02, 0.08, ed)
                            rgb = self.col_bg.xyz * 0.30
                                + self.col_a.xyz.mix(ftx.xyz * 1.6, clamp(cm * 1.1, 0.0, 1.0))
                                    * floor_line * gain * 0.5
                                + neon * 0.35 + glass + wcol * lit * inwin * 0.05
                        }
                    }
                }
            } } }
            let fmix = (1.0 - fogf) * fog_on
            let out = (rgb * self.fog.y).mix(self.col_bg.xyz, fmix)
            return vec4(out, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxCity {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = density add, p1 = sun/trail gain, p2 = beacon/edge gain.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (style, block pitch, street frac, fly).
    #[live(vec4(0.0, 6.0, 0.34, 1.0))]
    pub shape: Vec4f,
    /// (density, flicker, trail beats, bank).
    #[live(vec4(0.55, 0.12, 8.0, 1.0))]
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
    fn city_build_finite_and_classed() {
        let mut e = CityEngine::new(CityConfig { trails: 6, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(e.placed > 40, "placed {}", e.placed);
        assert!(mesh.triangle_count() > 500);
        let floats = super::super::mesh::VERT_FLOATS;
        let mut classes = [0usize; 4];
        for v in mesh.verts.chunks(floats) {
            for f in v {
                assert!(f.is_finite(), "non-finite vertex data");
            }
            let class = v[7] as usize;
            assert!(class < 4, "unknown class {}", v[7]);
            classes[class] += 1;
        }
        // All four classes present: towers, ground, sky, trails.
        for (i, n) in classes.iter().enumerate() {
            assert!(*n > 0, "class {i} missing from the stream");
        }
    }

    #[test]
    fn city_facade_uv_is_whole_window_units() {
        let mut e = CityEngine::new(CityConfig::default());
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        for v in mesh.verts.chunks(floats) {
            if v[7] != 0.0 || v[5] > 0.5 {
                continue; // towers only, facades only (normal.y == 0)
            }
            // Facade uv corners must sit on integer window counts.
            assert!((v[8] - v[8].round()).abs() < 1e-3, "u {} not whole", v[8]);
            assert!((v[9] - v[9].round()).abs() < 1e-3, "v {} not whole", v[9]);
        }
    }

    #[test]
    fn city_camera_stays_over_the_city_and_finite() {
        let e = CityEngine::new(CityConfig::default());
        for i in 0..200 {
            let cam = e.camera(i as f32 * 0.7);
            assert!(cam.eye.x.is_finite() && cam.eye.y.is_finite() && cam.eye.z.is_finite());
            // Lissajous axes are ±0.55·extent each, so the radius peaks at
            // 0.55·√2·extent ≈ 0.78·extent — still over the footprint.
            let r = (cam.eye.x * cam.eye.x + cam.eye.z * cam.eye.z).sqrt();
            assert!(r <= e.extent * 0.79, "camera left the city: {r} vs {}", e.extent);
            assert!(cam.eye.y > 0.0);
        }
    }

    #[test]
    fn city_degenerate_params_stay_safe() {
        let mut e = CityEngine::new(CityConfig {
            blocks: 0,
            block: f32::NAN,
            street: -4.0,
            max_h: f32::INFINITY,
            win: 0.0,
            towers: 0,
            trails: 99,
            fly: f32::NAN,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0);
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w] {
            assert!(v.is_finite(), "uniform not sanitized");
        }
        let cam = e.camera(3.0);
        assert!(cam.eye.y.is_finite());
    }
}
