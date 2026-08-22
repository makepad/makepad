//! Mountain-Jet — an endless fbm mountain range streaming under the camera
//! with a foreground fighter jet built from primitives: banked weaving
//! turns, an afterburner plume that pulses on the beat.
//!
//! The heightmap engine cannot host a foreground mesh (its vertex shader
//! displaces EVERYTHING by grid uv), so this is its sibling: one engine,
//! one static vertex stream carrying two families — the terrain grid
//! (heightmap technique: flat grid, fbm displacement + endless uv scroll in
//! the vertex shader) and the jet + afterburner primitives, told apart by
//! `a_aux` exactly like the firefly engine's blades/flies split. The jet is
//! placed in VIEW SPACE (always framed, whatever the camera), banking and
//! weaving from pure vertex-shader math; the plume is OPAQUE shaped
//! geometry (crossed diamond fans) so no blend-mode split is needed —
//! bloom stages make it glow.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!
//! Terrain (a_aux <= 1.0): a_id = checker cell, a_aux = 0, uv = grid uv,
//!   a_r0 = vertex hash, normal = +Y.
//! Jet hull (2.0 <= a_aux < 3.0): a_aux = 2.0 + part hash, geom_pos = LOCAL
//!   jet coords (nose = -z), normal = LOCAL face normal, uv = face uv
//!   (edge-wire material), a_r0 = per-face shade tint.
//! Burner (a_aux >= 4.0): a_aux = 4.0 + flicker seed, uv = (across, along
//!   the plume 0..1), a_r1 = along (stretch lever).
//!
//! # Document keys (`engine: "mountainjet"`)
//! * terrain: `res` (120), `size` (34), `height` (4.6), `noise_scale`
//!   (0.17), `scroll` (3.2), `ridged` (0.7), `cells` (40 grid density)
//! * look: `look` — `solid` (alpenglow shading) | `wire` (Battlezone
//!   vector) | `nightvision` (mono ramp + grain)
//! * jet: `jet_size` (1.0), `weave` (1.0 — bank/weave amplitude)
//! * bindings: `p0` ADDS afterburner gain (THE binding — `"0.3 + 2.0 *
//!   bass"`), `p1` scales weave rate, `p2` adds terrain glow
//! * hook: `fx_jet_color(shade, part, edge) -> vec4` — the hull material.

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

/// The look family, baked into `u_flow.y`.
#[derive(Clone, Copy, PartialEq)]
pub enum JetLook {
    Solid = 0,
    Wire = 1,
    NightVision = 2,
}

impl JetLook {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "solid" | "sunset" => Self::Solid,
            "wire" | "wireframe" | "battlezone" => Self::Wire,
            "nightvision" | "nv" | "night" => Self::NightVision,
            _ => return None,
        })
    }
}

pub struct JetConfig {
    /// Terrain grid resolution (quads per side).
    pub res: usize,
    /// World size of the terrain square.
    pub size: f32,
    pub height: f32,
    pub noise_scale: f32,
    /// Apparent flight speed (world units/sec of terrain stream).
    pub scroll: f32,
    /// 0 smooth hills, 1 ridged peaks.
    pub ridged: f32,
    /// Neon grid density on the terrain.
    pub cells: f32,
    pub look: JetLook,
    /// View-space scale of the jet model.
    pub jet_size: f32,
    /// Bank/weave amplitude 0..2.
    pub weave: f32,
    pub seed: u64,
}

impl Default for JetConfig {
    fn default() -> Self {
        Self {
            res: 120,
            size: 34.0,
            height: 4.6,
            noise_scale: 0.17,
            scroll: 3.2,
            ridged: 0.7,
            cells: 40.0,
            look: JetLook::Solid,
            jet_size: 1.0,
            weave: 1.0,
            seed: 29,
        }
    }
}

pub struct JetEngine {
    pub cfg: JetConfig,
    pub(crate) built: bool,
}

impl JetEngine {
    pub fn new(cfg: JetConfig) -> Self {
        Self { cfg, built: false }
    }

    /// Static build: terrain grid first, then the jet primitives, then the
    /// burner fans (index order = paint order is irrelevant here — this
    /// family is depth-tested opaque).
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let san = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let res = self.cfg.res.clamp(8, 220);
        let size = san(self.cfg.size, 34.0).clamp(4.0, 200.0);
        let mut rng = FxRng::new(self.cfg.seed);

        // -- terrain grid (heightmap technique) ---------------------------
        let mut rows: Vec<u32> = Vec::with_capacity((res + 1) * (res + 1));
        for zi in 0..=res {
            for xi in 0..=res {
                let u = xi as f32 / res as f32;
                let v = zi as f32 / res as f32;
                let pos = vec3f((u - 0.5) * size, 0.0, (v - 0.5) * size);
                let cell = ((xi / 4 + zi / 4) % 2) as f32;
                rows.push(mesh.push_vert(
                    pos,
                    cell,
                    vec3f(0.0, 1.0, 0.0),
                    0.0,
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

        // -- the jet (local coords, nose = -z, up = +y) -------------------
        self.build_jet(mesh);
    }

    /// One quad face of the hull. `part` 0..1 hashes the part family into
    /// a_aux (2.0 + part), `shade` is a per-face fixed tint (a_r0), uv runs
    /// 0..1 across the face so the pixel shader can draw its edges.
    #[allow(clippy::too_many_arguments)]
    fn face(mesh: &mut FxMesh, c: [Vec3f; 4], n: Vec3f, part: f32, shade: f32) {
        let uvs = [vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0)];
        let mut ids = [0u32; 4];
        for k in 0..4 {
            ids[k] = mesh.push_vert(c[k], 0.0, n, 2.0 + part, uvs[k], shade, 0.0);
        }
        mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
    }

    /// Loft 4 side faces between two rectangular cross sections at z0/z1
    /// (half-widths w, half-heights h) — the tapered-box workhorse.
    #[allow(clippy::too_many_arguments)]
    fn loft(
        mesh: &mut FxMesh,
        z0: f32,
        w0: f32,
        h0: f32,
        y0: f32,
        z1: f32,
        w1: f32,
        h1: f32,
        y1: f32,
        part: f32,
    ) {
        // top, bottom, right, left — stylized axis normals are fine for a
        // flat-shaded model.
        Self::face(
            mesh,
            [
                vec3f(-w0, y0 + h0, z0),
                vec3f(w0, y0 + h0, z0),
                vec3f(w1, y1 + h1, z1),
                vec3f(-w1, y1 + h1, z1),
            ],
            vec3f(0.0, 1.0, 0.0),
            part,
            1.0,
        );
        Self::face(
            mesh,
            [
                vec3f(-w0, y0 - h0, z0),
                vec3f(w0, y0 - h0, z0),
                vec3f(w1, y1 - h1, z1),
                vec3f(-w1, y1 - h1, z1),
            ],
            vec3f(0.0, -1.0, 0.0),
            part,
            0.45,
        );
        Self::face(
            mesh,
            [
                vec3f(w0, y0 - h0, z0),
                vec3f(w0, y0 + h0, z0),
                vec3f(w1, y1 + h1, z1),
                vec3f(w1, y1 - h1, z1),
            ],
            vec3f(1.0, 0.0, 0.0),
            part,
            0.72,
        );
        Self::face(
            mesh,
            [
                vec3f(-w0, y0 - h0, z0),
                vec3f(-w0, y0 + h0, z0),
                vec3f(-w1, y1 + h1, z1),
                vec3f(-w1, y1 - h1, z1),
            ],
            vec3f(-1.0, 0.0, 0.0),
            part,
            0.72,
        );
    }

    fn build_jet(&mut self, mesh: &mut FxMesh) {
        // Fuselage: three lofted segments, nose to tail.
        Self::loft(mesh, -0.95, 0.015, 0.015, 0.0, -0.45, 0.11, 0.095, 0.0, 0.0);
        Self::loft(mesh, -0.45, 0.11, 0.095, 0.0, 0.35, 0.145, 0.115, 0.0, 0.0);
        Self::loft(mesh, 0.35, 0.145, 0.115, 0.0, 0.75, 0.10, 0.095, 0.0, 0.0);
        // Tail cap.
        Self::face(
            mesh,
            [
                vec3f(-0.10, -0.095, 0.75),
                vec3f(0.10, -0.095, 0.75),
                vec3f(0.10, 0.095, 0.75),
                vec3f(-0.10, 0.095, 0.75),
            ],
            vec3f(0.0, 0.0, 1.0),
            0.0,
            0.35,
        );
        // Canopy (part 0.3 — the tinted one).
        Self::loft(mesh, -0.42, 0.055, 0.01, 0.095, -0.05, 0.075, 0.065, 0.10, 0.3);
        // Delta wings: one quad each (culling is off — visible both sides).
        Self::face(
            mesh,
            [
                vec3f(0.12, -0.02, -0.10),
                vec3f(0.98, -0.02, 0.58),
                vec3f(0.98, -0.02, 0.76),
                vec3f(0.12, -0.02, 0.66),
            ],
            vec3f(0.0, 1.0, 0.0),
            0.6,
            0.88,
        );
        Self::face(
            mesh,
            [
                vec3f(-0.12, -0.02, -0.10),
                vec3f(-0.98, -0.02, 0.58),
                vec3f(-0.98, -0.02, 0.76),
                vec3f(-0.12, -0.02, 0.66),
            ],
            vec3f(0.0, 1.0, 0.0),
            0.6,
            0.88,
        );
        // Horizontal stabilizers.
        Self::face(
            mesh,
            [
                vec3f(0.10, 0.01, 0.48),
                vec3f(0.46, 0.01, 0.70),
                vec3f(0.46, 0.01, 0.80),
                vec3f(0.10, 0.01, 0.78),
            ],
            vec3f(0.0, 1.0, 0.0),
            0.6,
            0.80,
        );
        Self::face(
            mesh,
            [
                vec3f(-0.10, 0.01, 0.48),
                vec3f(-0.46, 0.01, 0.70),
                vec3f(-0.46, 0.01, 0.80),
                vec3f(-0.10, 0.01, 0.78),
            ],
            vec3f(0.0, 1.0, 0.0),
            0.6,
            0.80,
        );
        // Twin canted tails (part 0.9).
        for side in [-1.0f32, 1.0] {
            Self::face(
                mesh,
                [
                    vec3f(side * 0.09, 0.09, 0.46),
                    vec3f(side * 0.09, 0.09, 0.76),
                    vec3f(side * 0.17, 0.42, 0.78),
                    vec3f(side * 0.17, 0.42, 0.64),
                ],
                vec3f(side, 0.0, 0.0),
                0.9,
                0.78,
            );
        }
        // Afterburner plume: crossed elongated diamond fans behind the
        // nozzle. OPAQUE shaped geometry — the shape IS the flame, the
        // vertex shader stretches it with the beat, bloom makes it glow.
        let mut rng = FxRng::new(self.cfg.seed.wrapping_add(7));
        for fan in 0..2 {
            let flick = rng.next_f32();
            let (wx, wy) = if fan == 0 { (0.11f32, 0.0f32) } else { (0.0, 0.11) };
            let pts = [
                (vec3f(0.0, 0.0, 0.78), vec2f(0.5, 0.0), 0.0f32),
                (vec3f(wx, wy, 1.00), vec2f(1.0, 0.28), 0.28),
                (vec3f(0.0, 0.0, 1.60), vec2f(0.5, 1.0), 1.0),
                (vec3f(-wx, -wy, 1.00), vec2f(0.0, 0.28), 0.28),
            ];
            let mut ids = [0u32; 4];
            for (k, (pos, uv, along)) in pts.iter().enumerate() {
                ids[k] = mesh.push_vert(
                    *pos,
                    0.0,
                    vec3f(0.0, 0.0, 1.0),
                    4.0 + flick,
                    *uv,
                    flick,
                    *along,
                );
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = |v: f32, d: f32| if v.is_finite() { v } else { d };
        EngineUniforms {
            shape: vec4(
                san(self.cfg.cells, 40.0).clamp(4.0, 160.0),
                san(self.cfg.height, 4.6).clamp(0.0, 40.0),
                san(self.cfg.noise_scale, 0.17).clamp(0.01, 4.0),
                san(self.cfg.ridged, 0.7).clamp(0.0, 1.0),
            ),
            flow: vec4(
                san(self.cfg.scroll, 3.2).clamp(0.0, 60.0),
                self.cfg.look as i32 as f32,
                san(self.cfg.jet_size, 1.0).clamp(0.05, 6.0),
                san(self.cfg.weave, 1.0).clamp(0.0, 2.0),
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Mountain-Jet: ONE depth-tested opaque shader, three vertex families by
    // a_aux (terrain <= 1.0, hull 2.x, burner 4.x). Terrain = the heightmap
    // displacement technique (fbm scrolled by time). The jet lives in VIEW
    // space: bank/weave/pitch are pure VS math, so it is always framed.
    // Three looks share the pixel shader: solid alpenglow / Battlezone
    // wire / night-vision (u_flow.y).
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxJet = set_type_default() do #(DrawVjFxJet::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_uv: varying(vec2f)
        // (family, shade/height, aux, dist-to-camera)
        v_misc: varying(vec4f)

        hash2: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(157.31, 113.97))) * 41739.613)
        }

        vnoise: fn(p: vec2) -> float {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (vec2(3.0, 3.0) - f * 2.0)
            let a = self.hash2(i)
            let b = self.hash2(i + vec2(1.0, 0.0))
            let c = self.hash2(i + vec2(0.0, 1.0))
            let d = self.hash2(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
        }

        // 3-octave fbm scrolled along v — the endless stream — with a soft
        // corridor down the middle so the jet always has a valley to fly.
        height_at: fn(uv: vec2) -> float {
            let scroll = vec2(0.0, self.time_beat.x * self.flow.x * 0.06)
            let p = (uv + scroll) * (34.0 * self.shape.z)
            let h1 = self.vnoise(p)
            let h2 = self.vnoise(p * 2.11 + vec2(13.0, 5.0))
            let h3 = self.vnoise(p * 4.19 + vec2(7.0, 19.0))
            let mut h = h1 * 0.58 + h2 * 0.30 + h3 * 0.12
            h = mix(h, 1.0 - abs(h * 2.0 - 1.0), self.shape.w)
            let corridor = smoothstep(0.03, 0.26, abs(uv.x - 0.5))
            return h * (0.25 + 0.75 * corridor)
        }

        // Document hook: the hull material.
        fx_jet_color: fn(shade: float, part: float, edge: float) -> vec4 {
            let mut base = self.col_c.xyz * (0.22 + 0.60 * shade)
            if part > 0.25 {
                if part < 0.45 {
                    // canopy tint
                    base = self.col_b.xyz * (0.35 + 0.55 * shade)
                }
            }
            return vec4(base + self.col_a.xyz * (edge * 0.5), 1.0)
        }

        vertex: fn() {
            let aux = self.geom.geom_pad
            let tm = self.time_beat.x
            if aux < 1.5 {
                // ---- TERRAIN ----
                let uv = self.geom.geom_uv
                let amp = self.shape.y * (1.0 + self.time_beat.w * 0.22)
                let h = self.height_at(uv)
                let e = 0.012
                let hx = self.height_at(uv + vec2(e, 0.0))
                let hz = self.height_at(uv + vec2(0.0, e))
                let pos = vec3(self.geom.geom_pos.x, h * amp, self.geom.geom_pos.z)
                let world = self.draw_list.view_transform
                    * vec4(pos.x, pos.y, pos.z, 1.0)
                let slope = clamp(1.0 - (abs(hx - h) + abs(hz - h)) * 9.0, 0.30, 1.0)
                // Sun-facing term from the x-gradient (alpenglow catches the
                // west faces).
                let sun = clamp(0.5 + (hx - h) * 26.0, 0.0, 1.0)
                let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
                let d = length(world.xyz - cam.xyz / max(cam.w, 0.0001))
                self.v_uv = uv
                self.v_misc = vec4(0.0, h, slope * (0.6 + 0.4 * sun), d)
                let view_pos = self.draw_pass.camera_view * world
                self.vertex_pos = self.draw_pass.camera_projection * view_pos
                return self.vertex_pos
            }
            // ---- JET + BURNER (view space) ----
            let scale = self.flow.z
            let weave = self.flow.w
            let ph = tm * 0.42 * clamp(1.0 + self.user.y, 0.1, 4.0)
            let xoff = sin(ph) * weave * 0.85
            let roll = (0.0 - cos(ph)) * clamp(weave * 0.55, 0.0, 1.2)
            let head = (0.0 - cos(ph)) * weave * 0.22
            let pitch = sin(tm * 0.27) * 0.05
            let mut local = self.geom.geom_pos * scale
            let mut nrml = self.geom.geom_normal
            if aux > 3.5 {
                // Burner stretch: the plume length breathes with the beat
                // (+ the p0 binding) and flickers per fan.
                let gain = clamp(
                    0.55 + self.time_beat.w * 1.1 + self.user.x
                        + sin(tm * 31.0 + fract(aux) * 47.0) * 0.10,
                    0.15, 3.5
                )
                let base_z = 0.78 * scale
                local = vec3(
                    local.x * (0.7 + gain * 0.4),
                    local.y * (0.7 + gain * 0.4),
                    base_z + (local.z - base_z) * gain
                )
            }
            // pitch (x), roll (z), heading (y) — then place in view space.
            let p1 = vec3(
                local.x,
                local.y * cos(pitch) - local.z * sin(pitch),
                local.y * sin(pitch) + local.z * cos(pitch)
            )
            let p2 = vec3(
                p1.x * cos(roll) - p1.y * sin(roll),
                p1.x * sin(roll) + p1.y * cos(roll),
                p1.z
            )
            let p3 = vec3(
                p2.x * cos(head) + p2.z * sin(head),
                p2.y,
                p2.z * cos(head) - p2.x * sin(head)
            )
            let n1 = vec3(
                nrml.x,
                nrml.y * cos(pitch) - nrml.z * sin(pitch),
                nrml.y * sin(pitch) + nrml.z * cos(pitch)
            )
            let n2 = vec3(
                n1.x * cos(roll) - n1.y * sin(roll),
                n1.x * sin(roll) + n1.y * cos(roll),
                n1.z
            )
            let bob = sin(tm * 0.9) * 0.07
            let base = vec3(xoff, 0.0 - 0.72 + bob, 0.0 - 6.4)
            let vp = base + p3
            let ldir = normalize(vec3(0.35, 0.78, 0.42))
            let shade = 0.30 + 0.70 * clamp(dot(n2, ldir), 0.0, 1.0)
            self.v_uv = self.geom.geom_uv
            if aux > 3.5 {
                self.v_misc = vec4(
                    2.0,
                    clamp(0.55 + self.time_beat.w * 1.1 + self.user.x, 0.15, 3.5),
                    fract(aux),
                    length(vp)
                )
            } else {
                self.v_misc = vec4(
                    1.0,
                    shade * self.geom.geom_tail_pad_0,
                    aux - 2.0,
                    length(vp)
                )
            }
            self.vertex_pos = self.draw_pass.camera_projection
                * vec4(vp.x, vp.y, vp.z, 1.0)
            return self.vertex_pos
        }

        pixel: fn() {
            let fam = self.v_misc.x
            let look = self.flow.y
            let fogf = exp(0.0 - self.v_misc.w * self.fog.x)
            let gain = self.fog.y * (1.0 + self.user.z)
            let mut rgb = vec3(0.0, 0.0, 0.0)
            if fam < 0.5 {
                // ---- TERRAIN ----
                let h = self.v_misc.y
                let lit = self.v_misc.z
                let scroll = vec2(0.0, self.time_beat.x * self.flow.x * 0.06)
                let g = fract((self.v_uv + scroll) * self.shape.x)
                let dg = min(min(g.x, 1.0 - g.x), min(g.y, 1.0 - g.y))
                let line = 1.0 - smoothstep(0.0, 0.11, dg)
                let beat_glow = 1.0 + self.time_beat.w * 0.9
                if look < 0.5 {
                    // solid alpenglow: valleys col_a, peaks col_b, summits
                    // catch col_c; a whisper of grid keeps the retro DNA.
                    let grad = self.col_a.xyz.mix(self.col_b.xyz, clamp(h * 1.5 - 0.15, 0.0, 1.0))
                    let glow = self.col_c.xyz * pow(clamp(h, 0.0, 1.0), 3.0) * 0.85
                    rgb = (grad * lit + glow + grad * line * 0.10) * gain
                } else {
                    if look < 1.5 {
                        // Battlezone wire: lines only over near-black fill.
                        let fill = self.col_a.xyz * 0.035
                        rgb = (fill + self.col_a.xyz * line * beat_glow
                            + self.col_b.xyz * (line * h * 1.3)) * gain
                    } else {
                        // night vision: mono luminance ramp + grain. The
                        // hash is INLINED: a helper fn bound to the vertex
                        // stage (hash2 feeds height_at) cannot also be
                        // called from the pixel stage.
                        let lum = clamp(0.14 + h * 0.9 * lit + line * 0.22, 0.0, 1.0)
                        let gv = floor(self.v_uv * 331.0)
                            + vec2(fract(self.time_beat.x * 17.0), 0.0)
                        let grain = fract(sin(dot(gv, vec2(157.31, 113.97))) * 41739.613) * 0.10
                        rgb = self.col_a.xyz * ((lum + grain) * beat_glow) * gain
                    }
                }
                return vec4(rgb.mix(self.col_bg.xyz, 1.0 - fogf), 1.0)
            }
            if fam < 1.5 {
                // ---- JET HULL ----
                let edge_d = min(
                    min(self.v_uv.x, 1.0 - self.v_uv.x),
                    min(self.v_uv.y, 1.0 - self.v_uv.y)
                )
                let edge = 1.0 - smoothstep(0.0, 0.09, edge_d)
                let shade = self.v_misc.y
                let part = self.v_misc.z
                if look < 0.5 {
                    let c = self.fx_jet_color(shade, part, edge)
                    rgb = c.xyz * gain
                } else {
                    if look < 1.5 {
                        // Battlezone: near-black silhouette body (the depth
                        // test cuts it into the grid) + hot vector edges,
                        // wider than the solid look so the outline reads
                        // against the busy terrain.
                        let we = 1.0 - smoothstep(0.0, 0.17, edge_d)
                        rgb = (self.col_a.xyz * (we * 2.4)
                            + self.col_c.xyz * (we * 0.5)
                            + self.col_a.xyz * 0.02) * gain
                    } else {
                        rgb = self.col_a.xyz * ((0.10 + shade * 0.55 + edge * 0.35)) * gain
                    }
                }
                return vec4(rgb.mix(self.col_bg.xyz, (1.0 - fogf) * 0.4), 1.0)
            }
            // ---- BURNER ----
            let along = self.v_uv.y
            let across = abs(self.v_uv.x - 0.5) * 2.0
            let core = pow(clamp(1.0 - along, 0.0, 1.0), 1.5)
                * clamp(1.0 - across, 0.0, 1.0)
            let heat = self.col_c.xyz.mix(self.col_b.xyz, along)
            let flick = 0.9 + 0.25 * sin(self.time_beat.x * 43.0 + self.v_misc.z * 61.0)
            rgb = heat * (core * (0.8 + self.v_misc.y * 1.6) * flick) * self.fog.y
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (the same 11 instance vec4s every fx shader
/// carries — the view's draw macro writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxJet {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = afterburner add, p1 = weave-rate scale, p2 = glow add.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (grid cells, height amp, noise scale, ridged).
    #[live(vec4(40.0, 4.6, 0.17, 0.7))]
    pub shape: Vec4f,
    /// (scroll, look 0..2, jet size, weave).
    #[live(vec4(3.2, 0.0, 1.0, 1.0))]
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
    fn jet_build_families_are_unambiguous() {
        let mut e = JetEngine::new(JetConfig { res: 16, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        let terrain_expected = 17 * 17;
        let (mut terrain, mut hull, mut burner) = (0usize, 0usize, 0usize);
        for v in mesh.verts.chunks(floats) {
            let aux = v[7];
            assert!(
                aux <= 1.0 || (2.0..3.0).contains(&aux) || (4.0..5.0).contains(&aux),
                "ambiguous family flag {aux}"
            );
            if aux <= 1.0 {
                terrain += 1;
            } else if aux < 3.0 {
                hull += 1;
            } else {
                burner += 1;
            }
        }
        assert_eq!(terrain, terrain_expected);
        assert!(hull >= 60, "the jet lost its hull: {hull} verts");
        assert_eq!(burner, 8, "two crossed burner fans = 8 verts");
    }

    #[test]
    fn jet_degenerate_params_stay_safe() {
        // Hostile doc values must clamp, not panic (frame-path law).
        let mut e = JetEngine::new(JetConfig {
            res: 0,
            size: f32::NAN,
            height: f32::INFINITY,
            noise_scale: -4.0,
            weave: f32::NAN,
            jet_size: 0.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0);
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w]
        {
            assert!(v.is_finite(), "non-finite uniform {v}");
        }
        assert!(u.flow.z >= 0.05, "jet size must keep a floor");
    }
}
