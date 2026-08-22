//! Firefly Synchrony — a dark meadow of fireflies pulled into sync by the
//! music (Kuramoto coupling collapsed to ONE mix uniform).
//!
//! Pure static-mesh class: the CPU uploads grass blades + firefly sprite
//! quads exactly once; every frame is vertex-shader work off per-vertex
//! seeds + the beat uniforms. The emotional arc is a single number:
//! `sync` 0 = every fly blinks on its own intrinsic clock (baked as random
//! phase/rate per fly), `sync` 1 = the whole field flashes as one on the
//! beat phase. Drive it with a binding (`p0` adds to the base sync) and a
//! quiet field of chaos becomes one heartbeat at the drop.
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!
//! Grass blade rows (a_aux <= 1.0):
//!   a_id   = row index (0 root, 1 mid, 2 tip)
//!   a_aux  = along-blade 0..1 (the wind lever)
//!   normal = lateral direction
//!   uv     = (side 0/1, per-blade phase hash)
//!   a_r0   = per-blade hue hash, a_r1 = half width
//!
//! Firefly sprite corners (a_aux >= 2.0):
//!   a_id   = fly index
//!   a_aux  = 2.0 + height class 0..1
//!   normal = ANCHOR POSITION (the home the fly wanders around)
//!   uv     = sprite corner 0/1
//!   a_r0   = intrinsic blink phase, a_r1 = intrinsic blink rate seed
//!
//! # Document keys (`engine: "firefly"`)
//! `flies` (700), `blades` (6000), `area` (8), `fly_height` (1.7),
//! `fly_size` (0.10), `sync` (0.35 — THE param; `p0` binding adds to it),
//! `blink_rate` (0.5 blinks/sec), `blink_sharp` (7), `wander` (0.55; `p1`
//! scales), `moon` (0.35 rim on the grass), `grass_height` (0.8),
//! `clump` (0.5). Shader hook: `fx_sprite(uv, tint)`.

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

pub struct FireflyConfig {
    pub flies: usize,
    pub blades: usize,
    /// Half-extent of the square meadow.
    pub area: f32,
    /// Vertical band the flies occupy above the grass.
    pub fly_height: f32,
    pub fly_size: f32,
    /// Base synchronization 0..1 (the shader adds the p0 binding).
    pub sync: f32,
    /// Intrinsic blinks per second (each fly's own rate varies ±40%).
    pub blink_rate: f32,
    /// Blink envelope sharpness (1 soft glow .. 24 strobe).
    pub blink_sharp: f32,
    /// Wander amplitude in world units (p1 binding scales).
    pub wander: f32,
    /// Moon rim light on the grass 0..1.
    pub moon: f32,
    pub grass_height: f32,
    pub clump: f32,
    pub seed: u64,
}

impl Default for FireflyConfig {
    fn default() -> Self {
        Self {
            flies: 700,
            blades: 6000,
            area: 8.0,
            fly_height: 1.7,
            fly_size: 0.10,
            sync: 0.35,
            blink_rate: 0.5,
            blink_sharp: 7.0,
            wander: 0.55,
            moon: 0.35,
            grass_height: 0.8,
            clump: 0.5,
            seed: 14,
        }
    }
}

pub struct FireflyEngine {
    pub cfg: FireflyConfig,
    pub(crate) built: bool,
}

impl FireflyEngine {
    pub fn new(cfg: FireflyConfig) -> Self {
        Self { cfg, built: false }
    }

    /// Static build: grass first (opaque overwrite), flies after (additive
    /// sprites always composite on top — the shader is depth-write-off, so
    /// index order IS the layering).
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let san = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let blades = self.cfg.blades.min(20_000);
        let flies = self.cfg.flies.clamp(8, 4000);
        let area = san(self.cfg.area, 8.0).clamp(2.0, 60.0);
        let grass_height = san(self.cfg.grass_height, 0.8).clamp(0.05, 4.0);
        let fly_height = san(self.cfg.fly_height, 1.7).clamp(0.1, 20.0);
        let mut rng = FxRng::new(self.cfg.seed);

        // -- grass ---------------------------------------------------------
        let mut clumps = [(0.0f32, 0.0f32); 24];
        for c in clumps.iter_mut() {
            *c = (rng.range(-area, area), rng.range(-area, area));
        }
        let clump = self.cfg.clump.clamp(0.0, 1.0);
        for _ in 0..blades {
            let (mut x, mut z) = (rng.range(-area, area), rng.range(-area, area));
            if rng.next_f32() < clump {
                let (cx, cz) = clumps[(rng.next_f32() * 24.0) as usize % 24];
                x = cx + rng.range(-1.2, 1.2);
                z = cz + rng.range(-1.2, 1.2);
            }
            let yaw = rng.range(0.0, std::f32::consts::TAU);
            let lean = rng.range(-0.14, 0.14);
            let h = grass_height * rng.range(0.55, 1.3);
            let w = 0.035 * rng.range(0.7, 1.2);
            let hue = rng.next_f32();
            let phase = rng.next_f32();
            let lateral = vec3f(yaw.cos(), 0.0, yaw.sin());
            let up = vec3f(lean, 1.0, lean * 0.6);
            let rows = [(0.0f32, w), (0.55, w * 0.55), (1.0, w * 0.03)];
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

        // -- fireflies -----------------------------------------------------
        let corners = [
            (vec2f(-0.5, -0.5), vec2f(0.0, 0.0)),
            (vec2f(0.5, -0.5), vec2f(1.0, 0.0)),
            (vec2f(0.5, 0.5), vec2f(1.0, 1.0)),
            (vec2f(-0.5, 0.5), vec2f(0.0, 1.0)),
        ];
        for i in 0..flies {
            let h01 = rng.next_f32();
            let anchor = vec3f(
                rng.range(-area, area),
                0.25 + h01 * fly_height,
                rng.range(-area, area),
            );
            let blink_phase = rng.next_f32();
            let blink_rate = rng.next_f32();
            let mut ids = [0u32; 4];
            for (k, (corner, uv)) in corners.iter().enumerate() {
                ids[k] = mesh.push_vert(
                    vec3f(corner.x, corner.y, 0.0),
                    i as f32,
                    anchor,
                    2.0 + h01,
                    *uv,
                    blink_phase,
                    blink_rate,
                );
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        // Rust's clamp propagates NaN — sanitize first so a hostile doc
        // value can never poison the uniform stream (panic/NaN-free law).
        let san = |v: f32, d: f32| if v.is_finite() { v } else { d };
        EngineUniforms {
            shape: vec4(
                san(self.cfg.sync, 0.35).clamp(0.0, 1.0),
                san(self.cfg.blink_sharp, 7.0).clamp(1.0, 24.0),
                san(self.cfg.fly_size, 0.10).clamp(0.005, 2.0),
                san(self.cfg.moon, 0.35).clamp(0.0, 2.0),
            ),
            flow: vec4(
                san(self.cfg.wander, 0.55).clamp(0.0, 20.0),
                san(self.cfg.blink_rate, 0.5).clamp(0.01, 8.0),
                san(self.cfg.area, 8.0).clamp(2.0, 60.0),
                0.0,
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Firefly Synchrony: ONE shader, two vertex families told apart by a_aux
    // (blade rows <= 1.0, fly sprites >= 2.0). Blades are opaque-overwrite
    // (alpha 1 under premultiplied blending), flies are additive sprites —
    // emitted after the grass, so with depth-write off they layer on top.
    // The sync law: blink phase = mix(intrinsic clock, beat phase, sync).
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxFirefly = set_type_default() do #(DrawVjFxFirefly::script_shader(vm)){
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
        v_uv: varying(vec2f)
        v_flag: varying(float)

        // Document hook: the firefly sprite (soft orb by default).
        fx_sprite: fn(uv: vec2, tint: vec4) -> vec4 {
            let d = length(uv - vec2(0.5, 0.5)) * 2.0
            let core = 1.0 - smoothstep(0.0, 0.35, d)
            let halo = 1.0 - smoothstep(0.08, 1.0, d)
            let a = clamp(core + halo * 0.6, 0.0, 1.0)
            return vec4(tint.xyz * a, 0.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            if attr.y > 1.5 {
                // ---- FIREFLY SPRITE ----
                let id = attr.x
                let anchor = self.geom.geom_normal
                let h01 = attr.y - 2.0
                let wt = self.time_beat.x
                // Wander: stateless curl-ish sum of sines per fly; p1 scales.
                let amp = self.flow.x * (0.35 + 0.65 * h01)
                    * clamp(1.0 + self.user.y, 0.0, 4.0)
                let off = vec3(
                    sin(wt * 0.41 + id * 1.93) + 0.5 * sin(wt * 0.97 + id * 4.13),
                    0.5 * sin(wt * 0.31 + id * 2.71),
                    cos(wt * 0.37 + id * 3.17) + 0.5 * cos(wt * 0.83 + id * 1.31)
                ) * amp
                let center = anchor + off
                // THE sync law. time_beat.z = beat phase.
                let sync = clamp(self.shape.x + self.user.x, 0.0, 1.0)
                let intrinsic = fract(
                    attr.z + self.time_beat.x * self.flow.y * (0.6 + 0.8 * attr.w)
                )
                let b = mix(intrinsic, self.time_beat.z, sync)
                let sharp = clamp(self.shape.y, 1.0, 24.0)
                let bright = pow(1.0 - b, sharp)
                let world = self.draw_list.view_transform
                    * vec4(center.x, center.y, center.z, 1.0)
                let view_pos = self.draw_pass.camera_view * world
                let size = self.shape.z * (0.55 + 0.65 * h01) * (0.45 + 1.9 * bright)
                let billboard = vec4(
                    view_pos.x + self.geom.geom_pos.x * size,
                    view_pos.y + self.geom.geom_pos.y * size,
                    view_pos.z,
                    view_pos.w
                )
                let body = self.col_b.mix(self.col_c, bright * bright)
                let gain = (0.05 + 2.4 * bright) * self.fog.y
                self.v_color = vec4(body.xyz * gain, 1.0)
                self.v_uv = self.geom.geom_uv
                self.v_flag = 0.0
                self.vertex_pos = self.draw_pass.camera_projection * billboard
                return self.vertex_pos
            }
            // ---- GRASS BLADE ----
            let along = attr.y
            let wf = self.time_beat.x * self.anim.y
            let bend = self.anim.x * along * along * 0.5
            let pos = self.geom.geom_pos + vec3(
                sin(wf + self.geom.geom_pos.x * 0.35 + self.geom.geom_pos.z * 0.22),
                0.0,
                cos(wf * 0.81 + self.geom.geom_pos.z * 0.33)
            ) * bend
            let world = self.draw_list.view_transform * vec4(pos.x, pos.y, pos.z, 1.0)
            let base = self.col_a.mix(self.col_b, along * 0.7 + attr.z * 0.15)
            let moon = self.shape.w * (0.25 + 0.75 * along)
            let breath = 1.0 + self.time_beat.w * 0.18
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let d = length(world.xyz - cam.xyz / max(cam.w, 0.0001))
            let fogf = exp(0.0 - d * self.fog.x)
            let lit = base.xyz * ((0.16 + moon) * breath * self.fog.y)
            let rgb = lit.mix(self.col_bg.xyz, 1.0 - fogf)
            self.v_color = vec4(rgb, 1.0)
            self.v_uv = self.geom.geom_uv
            self.v_flag = 1.0
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            if self.v_flag > 0.5 {
                // Blade: opaque overwrite under premultiplied blending.
                return vec4(self.v_color.xyz, 1.0)
            }
            return self.fx_sprite(self.v_uv, self.v_color)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (the same 11 instance vec4s every fx shader
/// carries — see shaders.rs; the view's draw macro writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxFirefly {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = sync add, p1 = wander scale add.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (sync base, blink_sharp, fly_size, moon).
    #[live(vec4(0.35, 7.0, 0.1, 0.35))]
    pub shape: Vec4f,
    /// (wander amp, blink rate, area, unused).
    #[live(vec4(0.55, 0.5, 8.0, 0.0))]
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
    fn firefly_build_counts_and_channels() {
        let mut e = FireflyEngine::new(FireflyConfig {
            blades: 100,
            flies: 50,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        // 100 blades * 6 verts + 50 flies * 4 verts.
        assert_eq!(mesh.vertex_count(), 100 * 6 + 50 * 4);
        // Every fly vertex flags a_aux >= 2.0; every blade vertex < 1.5.
        let floats = super::super::mesh::VERT_FLOATS;
        let mut fly_verts = 0;
        for v in mesh.verts.chunks(floats) {
            let aux = v[7];
            assert!(aux <= 1.0 || aux >= 2.0, "ambiguous family flag {aux}");
            if aux >= 2.0 {
                fly_verts += 1;
                // Anchor position must be finite and inside the meadow band.
                assert!(v[4].abs() <= e.cfg.area + 0.001);
                assert!(v[5] > 0.0 && v[5] < e.cfg.fly_height + 0.3);
            }
        }
        assert_eq!(fly_verts, 50 * 4);
    }

    #[test]
    fn firefly_degenerate_params_stay_safe() {
        // Hostile doc values must clamp, not panic (frame-path law).
        let mut e = FireflyEngine::new(FireflyConfig {
            blades: 0,
            flies: 0,
            area: -5.0,
            fly_height: 0.0,
            grass_height: -1.0,
            ..Default::default()
        });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert!(mesh.vertex_count() > 0, "fly floor keeps the effect alive");
        let u = e.uniforms();
        assert!(u.shape.x.is_finite() && u.flow.z.is_finite());
    }
}
