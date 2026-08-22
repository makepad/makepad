//! Kick Forge — a drumhead piled with metal shards; every kick launches the
//! pile ballistically. Shards rise, tumble, glint and rain back onto a
//! membrane that rings with a damped radial wave from the same impulse.
//!
//! ENTIRELY STATELESS physics: the mesh is static (built once) and every
//! frame the vertex shader recomputes each shard's position as a pure
//! function of (beat phase, bpm, per-shard seeds). Per pulse, `t = phase *
//! seconds_per_pulse`; launch velocity is re-hashed with the pulse index so
//! every hit scatters differently; height = `v·t − ½g·t²` clamped to the
//! pile, and because the impulse gain is sampled per frame from the music
//! binding (bass/pulse), a fading kick pulls its own shards back down —
//! nothing integrates, nothing drifts, nothing can panic.
//!
//! Continuity laws (why nothing pops):
//! * lateral drift ∝ ballistic height — exactly 0 at launch and landing
//! * tumble angle is gated by `smoothstep(height)` — it unwinds smoothly in
//!   the last centimetres instead of snapping at touchdown
//! * landed shards RIDE the membrane wave through the same continuous
//!   `a_aux` (distance from centre) the membrane itself displaces by
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//! Shards (one camera-agnostic quad per shard, expanded in the VS from a
//! hashed tumbling 3D frame — real facet normals, real glints):
//!   geom_pos = shard REST CENTRE on the pile
//!   a_id     = shard index (>= 0)
//!   normal   = launch direction seed (unit, up-biased; scatter baked in)
//!   a_aux    = distance from drum centre 0..1 (impulse falloff + membrane)
//!   uv       = quad corner 0/1
//!   a_r0     = launch gain seed (impulse falloff BAKED at build)
//!   a_r1     = spin/size seed
//! Membrane (disc grid):
//!   geom_pos = rest position (y = 0)   a_id = −1 (the membrane marker)
//!   normal   = +Y      a_aux = radial 0..1     uv = (angle01, radial01)
//!   a_r0     = vertex hash              a_r1 = 0
//!
//! # Document keys (`engine: "forge"`)
//! `shards` (2000, ≤6000), `radius` (4.0), `impulse` (7.0 launch speed),
//! `gravity` (42.0 — HIGH gravity = high jumps: launches are capped to
//! land before the next hit, so the reachable height is `g·T²/8`),
//! `spin` (1.0 tumble rate), `membrane_wave` (0.5), `shard_size` (0.16),
//! `scatter` (0.55 lateral cone), `falloff` (0.55 — impulse fade towards
//! the rim), `pile` (0.55 centre pile height), `auto_pump` (1 — a constant
//! launch-gain floor so the forge fires on every free-running beat; set 0
//! for silence-still once real audio is wired), `glint` (1.0). Bindings:
//! `p0` = impulse gain (THE binding — `"0.6 + 2.6*bass"`), `p1` = extra
//! membrane wave gain, `p2` = glint boost (hats), `p3` free. Hook:
//! `fx_color(t = flight heat 0..1, attr = (id, aux, r0, r1), normal =
//! facet, wpos)`.
//!
//! Content coupling (`content:` → `fog.z`): the flat shard facets mirror
//! the channel video (one vertex-stage env sample per facet), the
//! reflection swinging as they tumble.

use super::engines::EngineUniforms;
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

pub struct ForgeConfig {
    pub shards: usize,
    /// Drum radius (world units).
    pub radius: f32,
    /// Base launch speed at full gain.
    pub impulse: f32,
    pub gravity: f32,
    /// Tumble rate scale.
    pub spin: f32,
    /// Membrane wave amplitude scale.
    pub membrane_wave: f32,
    pub shard_size: f32,
    /// Lateral launch cone 0..~1 (baked into the direction seeds).
    pub scatter: f32,
    /// Impulse fade towards the rim 0..1 (baked into the gain seeds).
    pub falloff: f32,
    /// Pile height at the centre.
    pub pile: f32,
    /// Pulse-driven fallback gain when p0 is unbound/zero (0 = silence-still).
    pub auto_pump: f32,
    /// Glint gain.
    pub glint: f32,
    /// Pulses per beat (mirrors the document's beat_rate — the shader needs
    /// it to reconstruct seconds-per-pulse and the pulse index).
    pub rate: f32,
    pub seed: u64,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            shards: 2000,
            radius: 4.0,
            impulse: 7.0,
            gravity: 42.0,
            spin: 1.0,
            membrane_wave: 0.5,
            shard_size: 0.16,
            scatter: 0.55,
            falloff: 0.55,
            pile: 0.55,
            auto_pump: 1.0,
            glint: 1.0,
            rate: 1.0,
            seed: 11,
        }
    }
}

/// Membrane tessellation (constants — the radius scales the same grid).
const MEM_RINGS: usize = 30;
const MEM_SECTORS: usize = 64;

pub struct ForgeEngine {
    pub cfg: ForgeConfig,
    pub(crate) built: bool,
    /// Shards actually placed (after clamping).
    pub placed: usize,
}

impl ForgeEngine {
    pub fn new(cfg: ForgeConfig) -> Self {
        Self { cfg, built: false, placed: 0 }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    /// Static build: the membrane disc + one quad per shard. Every quad's
    /// four vertices share the rest centre; the shader expands the corners
    /// through a hashed tumbling frame (per-face normals for free).
    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let radius = Self::san(self.cfg.radius, 4.0).clamp(1.0, 40.0);
        let scatter = Self::san(self.cfg.scatter, 0.55).clamp(0.0, 2.0);
        let falloff = Self::san(self.cfg.falloff, 0.55).clamp(0.0, 1.0);
        let pile = Self::san(self.cfg.pile, 0.55).clamp(0.0, 4.0);
        let mut rng = FxRng::new(self.cfg.seed);

        // -- membrane disc (a_id = −1 marks it for the shader) -------------
        let mut ring_ids: Vec<Vec<u32>> = Vec::with_capacity(MEM_RINGS + 1);
        for ri in 0..=MEM_RINGS {
            let r01 = ri as f32 / MEM_RINGS as f32;
            let mut ids = Vec::with_capacity(MEM_SECTORS + 1);
            for si in 0..=MEM_SECTORS {
                let a01 = si as f32 / MEM_SECTORS as f32;
                let a = a01 * std::f32::consts::TAU;
                let pos = vec3f(a.cos() * r01 * radius, 0.0, a.sin() * r01 * radius);
                ids.push(mesh.push_vert(
                    pos,
                    -1.0,
                    vec3f(0.0, 1.0, 0.0),
                    r01,
                    vec2f(a01, r01),
                    rng.next_f32(),
                    0.0,
                ));
            }
            ring_ids.push(ids);
        }
        for ri in 0..MEM_RINGS {
            for si in 0..MEM_SECTORS {
                mesh.push_quad(
                    ring_ids[ri][si],
                    ring_ids[ri][si + 1],
                    ring_ids[ri + 1][si + 1],
                    ring_ids[ri + 1][si],
                );
            }
        }

        // -- shards --------------------------------------------------------
        let count = self.cfg.shards.clamp(64, 6000);
        for i in 0..count {
            // Centre-dense scatter over the disc (k < 0.5-uniform exponent).
            let r01 = rng.next_f32().powf(0.72);
            let ang = rng.range(0.0, std::f32::consts::TAU);
            let rest = vec3f(
                ang.cos() * r01 * radius,
                pile * (1.0 - r01).powf(1.7) * (0.75 + 0.5 * rng.next_f32()) + 0.03,
                ang.sin() * r01 * radius,
            );
            // Launch direction seed: up-biased, radially outward, scatter
            // baked so it costs no uniform slot.
            let tilt = rng.range(0.15, 1.0) * scatter;
            let jx = rng.range(-0.3, 0.3) * scatter;
            let jz = rng.range(-0.3, 0.3) * scatter;
            let d = vec3f(ang.cos() * tilt + jx, 1.0, ang.sin() * tilt + jz);
            let dl = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-4);
            let dir = vec3f(d.x / dl, d.y / dl, d.z / dl);
            // Gain seed with the rim falloff baked in.
            let gain = ((0.35 + 0.65 * rng.next_f32()) * (1.0 - falloff * r01)).max(0.05);
            let spin_size = rng.next_f32();
            let id = i as f32;
            let corners = [vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0)];
            let mut ids = [0u32; 4];
            for (k, uv) in corners.iter().enumerate() {
                ids[k] = mesh.push_vert(rest, id, dir, r01, *uv, gain, spin_size);
            }
            mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
        }
        self.placed = count;
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        EngineUniforms {
            shape: vec4(
                san(self.cfg.rate, 1.0).clamp(0.05, 8.0),
                san(self.cfg.gravity, 15.0).clamp(0.5, 100.0),
                san(self.cfg.impulse, 7.0).clamp(0.0, 60.0),
                san(self.cfg.spin, 1.0).clamp(0.0, 8.0),
            ),
            flow: vec4(
                san(self.cfg.membrane_wave, 0.5).clamp(0.0, 4.0),
                san(self.cfg.shard_size, 0.16).clamp(0.01, 1.5),
                san(self.cfg.auto_pump, 1.0).clamp(0.0, 2.0),
                san(self.cfg.glint, 1.0).clamp(0.0, 8.0),
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Kick Forge: solid depth-tested chips + membrane, one draw. All shading
    // happens per VERTEX (chips are flat quads — flat-facet lighting is
    // exact there); the pixel stage only adds chip edge wear and membrane
    // hairline rings, keeping the budget small.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxForge = set_type_default() do #(DrawVjFxForge::script_shader(vm)){
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

        v_color: varying(vec4f)
        v_uv: varying(vec2f)
        // (is_membrane, ring01 for hairlines, unused, unused)
        v_extra: varying(vec4f)

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

        // The membrane's damped radial wave, seconds after the hit. Landed
        // shards ride the SAME function of the SAME continuous a_aux.
        memwave: fn(r01: float, ts: float) -> float {
            return sin(r01 * 24.0 - ts * 30.0)
                * exp(0.0 - r01 * 2.4) * exp(0.0 - ts * 6.5)
        }

        // Document hook: shard material. t = flight heat (1 at launch,
        // cooling to 0 by the next hit; 0 at rest), attr = (id, aux, r0, r1).
        fx_color: fn(t: float, attr: vec4, normal: vec3, wpos: vec3) -> vec4 {
            let base = self.col_a.mix(self.col_b, fract(attr.w * 7.93))
            let flash = self.col_c * (t * 0.55)
            return vec4(base.xyz + flash.xyz, 1.0)
        }

        vertex: fn() {
            let attr = vec4(
                self.geom.geom_id,
                self.geom.geom_pad,
                self.geom.geom_tail_pad_0,
                self.geom.geom_tail_pad_1
            )
            // The shared music clock: seconds since the pulse + pulse index,
            // reconstructed from (beat, phase, bpm, rate) — all uniforms.
            let rate = max(self.shape.x, 0.05)
            let secs_per = 60.0 / max(self.sig.y * rate, 0.5)
            let phase = self.time_beat.z
            let ts = phase * secs_per
            let hit = floor(self.time_beat.y * rate - phase + 0.5)
            // Impulse gain: the music binding (p0) with the auto_pump floor.
            // The floor is CONSTANT within a cycle on purpose — the punch
            // comes from the ballistics resetting at each hit; a gain that
            // decayed in-flight would sag every arc into a shiver.
            let gain = clamp(max(self.user.x, self.flow.z), 0.0, 6.0)
            let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            let cam_pos = cam.xyz / max(cam.w, 0.0001)

            let mut wpos = vec3(0.0, 0.0, 0.0)
            let mut rgb = vec3(0.0, 0.0, 0.0)
            let mut is_mem = 0.0
            if attr.x < -0.5 {
                // ---- membrane ------------------------------------------
                is_mem = 1.0
                let wamp = self.flow.x * clamp(gain + self.user.y, 0.0, 8.0)
                let w = self.memwave(attr.y, ts)
                wpos = self.geom.geom_pos + vec3(0.0, w * wamp, 0.0)
                let ringlight = clamp(abs(w) * (0.25 + wamp * 1.5), 0.0, 1.5)
                let base = self.col_bg.xyz * 1.5 + self.col_a.xyz * 0.06
                rgb = base + self.col_b.xyz * ringlight * self.fog.y
            } else {
                // ---- shard ---------------------------------------------
                let id = attr.x
                let hs = self.hash1(id * 0.618 + hit * 37.0)
                let g = max(self.shape.y, 0.5)
                // Velocity cap (per-shard jittered): every shard is DOWN
                // again before the next hit — no mid-air teleports, ever.
                let vcap = g * secs_per * 0.475 * (0.55 + 0.45 * hs)
                let v0 = min(
                    self.shape.z * attr.z * (0.55 + 0.9 * hs) * gain,
                    vcap
                )
                let yb = max(v0 * ts - 0.5 * g * ts * ts, 0.0)
                let airk = smoothstep(0.0, 0.05, yb)
                // Drift ∝ height: 0 at launch AND landing — no pops.
                let dirn = self.geom.geom_normal
                let drift = vec3(dirn.x, 0.0, dirn.z) * (yb * 0.8)
                // Landed shards ride the membrane through the same field.
                let ride = self.memwave(attr.y, ts)
                    * self.flow.x * clamp(gain + self.user.y, 0.0, 8.0)
                    * 0.7 * (1.0 - airk)
                let center = self.geom.geom_pos + drift
                    + vec3(0.0, yb + ride, 0.0)
                // Hashed rest frame + tumble around a hashed axis. The
                // airk gate unwinds the tumble smoothly at touchdown.
                let m0r = vec3(
                    self.hash1(id * 3.17 + 1.0) - 0.5,
                    self.hash1(id * 5.29 + 2.0) - 0.5 + 0.02,
                    self.hash1(id * 7.51 + 3.0) - 0.5
                )
                let m0 = m0r / max(length(m0r), 0.05)
                let axr = vec3(
                    self.hash1(id * 11.3 + 4.0) - 0.5 + 0.02,
                    self.hash1(id * 13.7 + 5.0) - 0.5,
                    self.hash1(id * 17.9 + 6.0) - 0.5
                )
                let axis = axr / max(length(axr), 0.05)
                let p0r = self.xcross(m0, vec3(0.0, 1.0, 0.0))
                    + vec3(0.017, 0.0, 0.011)
                let p0o = p0r - m0 * dot(m0, p0r)
                let p0 = p0o / max(length(p0o), 0.02)
                let q0 = self.xcross(m0, p0)
                let spin_eff = self.shape.w * (0.4 + 1.6 * attr.w)
                    * clamp(v0 / max(self.shape.z, 0.01), 0.0, 2.0) * 5.0
                let theta = self.hash1(id * 19.3 + 7.0) * 6.2831853
                    + spin_eff * ts * airk
                let c = cos(theta)
                let s = sin(theta)
                let p = self.rodr(p0, axis, c, s)
                let q = self.rodr(q0, axis, c, s)
                let m = self.rodr(m0, axis, c, s)
                let sx = self.flow.y * (0.55 + 0.9 * fract(attr.w * 17.13))
                let sy = sx * 0.65
                let corner = self.geom.geom_uv - vec2(0.5, 0.5)
                wpos = center + p * (corner.x * sx) + q * (corner.y * sy)
                // Flat-facet metal shading, computed per vertex (exact for
                // a flat quad): key light + broad spec + narrow glint.
                let key = normalize(vec3(0.45, 0.8, 0.35))
                let vdir = normalize(cam_pos - center)
                let hvec = normalize(key + vdir)
                let diff = abs(dot(m, key))
                let ndh = abs(dot(m, hvec))
                let spec = pow(ndh, 28.0)
                let glint = pow(ndh, 120.0)
                    * self.flow.w * clamp(1.0 + self.user.z * 3.0, 0.0, 8.0)
                let heat = airk * (1.0 - phase)
                let tint = self.fx_color(heat, attr, m, wpos)
                // CONTENT: the shards become MIRRORS of the channel video.
                // The mirror-direction env map (the first pass) squeezed a
                // whole frame into a few degrees of facet normal and read
                // as a glint; sampling at the shard's own SCREEN position,
                // nudged by the facet normal, makes each shard reflect what
                // is behind it — a pile of mirror fragments assembling the
                // picture over the shared video backdrop. Still ONE sample
                // per vertex (exact for a flat quad).
                // fog.z = the pre-gated `content` strength.
                let cm = self.fog.z
                let sclip = self.draw_pass.camera_projection
                    * (self.draw_pass.camera_view
                        * (self.draw_list.view_transform
                            * vec4(wpos.x, wpos.y, wpos.z, 1.0)))
                let sndc = sclip.xy / max(sclip.w, 0.0001)
                let suv = clamp(
                    vec2(sndc.x * 0.5 + 0.5, 0.5 - sndc.y * 0.5)
                        + vec2(m.x, 0.0 - m.y) * 0.11,
                    vec2(0.0, 0.0),
                    vec2(1.0, 1.0)
                )
                let env = self.tex0.sample_nearest(suv, 0.0)
                let facet = tint.xyz.mix(
                    env.xyz * (0.55 + 0.65 * diff),
                    clamp(cm * 1.25, 0.0, 1.0)
                )
                rgb = facet * ((0.30 + 0.70 * diff) * self.fog.y)
                    + self.col_c.xyz * (spec * 0.5 + glint)
            }

            let world = self.draw_list.view_transform
                * vec4(wpos.x, wpos.y, wpos.z, 1.0)
            let d = length(world.xyz - cam_pos)
            let fogf = exp(0.0 - d * self.fog.x)
            self.v_color = vec4(rgb.mix(self.col_bg.xyz, 1.0 - fogf), 1.0)
            self.v_uv = self.geom.geom_uv
            self.v_extra = vec4(is_mem, self.geom.geom_pad, 0.0, 0.0)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
            return self.vertex_pos
        }

        pixel: fn() {
            if self.v_extra.x > 0.5 {
                // Membrane: faint concentric hairlines give the drumhead
                // structure without a second draw.
                let rr = fract(self.v_extra.y * 16.0)
                let line = pow(1.0 - min(rr, 1.0 - rr) * 2.0, 24.0) * 0.10
                return vec4(self.v_color.xyz + self.col_a.xyz * line, 1.0)
            }
            // Chips: edge wear — darker rim reads as thickness.
            let e = min(
                min(self.v_uv.x, 1.0 - self.v_uv.x),
                min(self.v_uv.y, 1.0 - self.v_uv.y)
            )
            let rim = 0.72 + 0.28 * smoothstep(0.0, 0.16, e)
            return vec4(self.v_color.xyz * rim, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxForge {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = impulse gain (THE binding), p1 = membrane gain add,
    /// p2 = glint boost, p3 free.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (beat_rate, gravity, impulse, spin).
    #[live(vec4(1.0, 42.0, 7.0, 1.0))]
    pub shape: Vec4f,
    /// (membrane_wave, shard_size, auto_pump, glint).
    #[live(vec4(0.5, 0.16, 1.0, 1.0))]
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

    const MEM_VERTS: usize = (MEM_RINGS + 1) * (MEM_SECTORS + 1);

    #[test]
    fn forge_build_counts_and_finite() {
        let mut e = ForgeEngine::new(ForgeConfig { shards: 500, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(e.placed, 500);
        assert_eq!(mesh.vertex_count(), MEM_VERTS + 500 * 4);
        for v in mesh.verts.chunks(super::super::mesh::VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "non-finite vertex data");
            }
        }
    }

    #[test]
    fn forge_shard_seeds_launch_upward_with_positive_gain() {
        let mut e = ForgeEngine::new(ForgeConfig { shards: 300, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        for v in mesh.verts.chunks(floats).skip(MEM_VERTS) {
            assert!(v[3] >= 0.0, "shard ids must be >= 0");
            assert!(v[5] > 0.0, "launch dir must point up (dir.y > 0)");
            let dl = (v[4] * v[4] + v[5] * v[5] + v[6] * v[6]).sqrt();
            assert!((dl - 1.0).abs() < 1e-3, "launch dir must be unit");
            assert!(v[10] > 0.0 && v[10] <= 1.2, "gain seed out of range: {}", v[10]);
            assert!((0.0..=1.0).contains(&v[7]), "aux must be radial 0..1");
        }
    }

    #[test]
    fn forge_membrane_marked_and_flat() {
        let mut e = ForgeEngine::new(ForgeConfig { shards: 100, ..Default::default() });
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        let floats = super::super::mesh::VERT_FLOATS;
        for v in mesh.verts.chunks(floats).take(MEM_VERTS) {
            assert_eq!(v[3], -1.0, "membrane must be marked a_id = -1");
            assert_eq!(v[1], 0.0, "membrane rest y must be 0");
        }
    }

    #[test]
    fn forge_degenerate_params_stay_safe() {
        let mut e = ForgeEngine::new(ForgeConfig {
            shards: 0,
            radius: f32::NAN,
            impulse: -5.0,
            gravity: f32::INFINITY,
            spin: f32::NAN,
            membrane_wave: -1.0,
            shard_size: 0.0,
            scatter: 99.0,
            falloff: 3.0,
            pile: -2.0,
            rate: f32::NAN,
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
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w]
        {
            assert!(v.is_finite(), "uniform not sanitized");
        }
        assert!(u.shape.x >= 0.05, "rate clamped");
        assert!(u.shape.z >= 0.0, "impulse clamped non-negative");
    }
}
