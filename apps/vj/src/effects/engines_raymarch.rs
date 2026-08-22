//! Raymarch — a fullscreen SDF raymarcher whose SCENE is a subclassable
//! shader hook. The engine uploads ONE quad; everything else is the pixel
//! shader: sphere-trace the distance field, shade with a normal estimate,
//! cheap AO, an optional soft shadow, a cosine palette (own constants), and
//! exponential fog to the document background. Clean-room technique only —
//! domain repetition / folds / smooth-min are public-domain math; no
//! transliterated shader entries.
//!
//! # The subclass contract (THE variant mechanism)
//!
//! A preset overrides ONE function and inherits the whole marcher:
//!
//! ```text
//! shader: draw.DrawVjFxRaymarch {
//!     scene_sdf: fn(p: vec3) -> vec2 {
//!         // return (signed distance, material id)
//!         // material >= 0.0  -> solid, fed to fx_palette
//!         // material <  0.0  -> GLASS: the ray refracts once (Snell) and
//!         //                     samples input texture 0 — the optics family
//!         let d = length(p) - 1.0
//!         return vec2(d, 0.5)
//!     }
//! }
//! ```
//!
//! Base helpers a scene may call: `self.sd_box(p, b)`, `self.rep1(x, c)`
//! (floor-based domain repetition — safe for negative coordinates on every
//! backend), `self.rot2(p, a)`, `self.smin(a, b, k)`. The beat block is in
//! scope inside the hook (`self.time_beat`, `self.user`, `self.anim`…), so
//! deformation lives in the scene, driven by bindings. `fx_palette(t)` is
//! the second hook (cosine palette by default, steered by col_a/col_b).
//!
//! # Document keys (`engine: "raymarch"`)
//! * `steps` (64, 16..120) — THE march budget; the honest cost dial.
//! * `max_dist` (40) — ray horizon.
//! * `cam` — `orbit` (default) | `fly` (endless forward) | `dolly`
//!   (beat-bounced push-in), `cam_speed` (0.25), `cam_dist` (7),
//!   `cam_height` (2.2), `cam_fov` (60).
//! * `shadow` (1) — 0 disables the soft-shadow march (the cheap mode).
//! * Global deformation: `twist` (shared anim key) twists the whole field
//!   around Y; the distance estimate is Lipschitz-compensated.
//! * Bindings: `p0`/`p1`/`p2` are free scene levers (presets document
//!   theirs); `p3` bends the glass IOR in the optics family.
//!
//! Full-res honesty: there is no reduced-resolution internal pass in the
//! render graph today (the scene pass runs at output size), so the budget
//! lives in `steps` — presets ship with conservative counts and the
//! measured costs are reported in MANIFEST.md.
//!
//! Content coupling (`content:` → `fog.z`): solid materials project the
//! channel video planar by the dominant normal axis (floors/walls become
//! frescoes, lit and occluded like the material). The glass family already
//! refracts input0 and is untouched.

use super::engines::EngineUniforms;
use super::mesh::FxMesh;
use makepad_widgets::*;

/// Camera motion program, baked into `u_shape.z`.
#[derive(Clone, Copy, PartialEq)]
pub enum RaymarchCam {
    Orbit = 0,
    Fly = 1,
    Dolly = 2,
}

impl RaymarchCam {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "orbit" => Self::Orbit,
            "fly" => Self::Fly,
            "dolly" => Self::Dolly,
            _ => return None,
        })
    }
}

pub struct RaymarchConfig {
    /// March budget (iteration cap in the sphere-trace loop).
    pub steps: usize,
    pub max_dist: f32,
    pub cam: RaymarchCam,
    /// Orbit: radians/sec. Fly: world units/sec. Dolly: bounce rate.
    pub cam_speed: f32,
    pub cam_dist: f32,
    pub cam_height: f32,
    pub fov: f32,
    /// 1 = soft-shadow march on, 0 = off (the cheap mode).
    pub shadow: f32,
}

impl Default for RaymarchConfig {
    fn default() -> Self {
        Self {
            steps: 64,
            max_dist: 40.0,
            cam: RaymarchCam::Orbit,
            cam_speed: 0.25,
            cam_dist: 7.0,
            cam_height: 2.2,
            fov: 60.0,
            shadow: 1.0,
        }
    }
}

pub struct RaymarchEngine {
    pub cfg: RaymarchConfig,
    pub(crate) built: bool,
}

impl RaymarchEngine {
    pub fn new(cfg: RaymarchConfig) -> Self {
        Self { cfg, built: false }
    }

    /// One fullscreen quad in CLIP SPACE — the vertex shader passes the
    /// corners straight through; the pixel shader is the whole effect.
    /// uv (0,0) is the TOP-left corner (clip y = +1) so `v_uv` reads like a
    /// screen texture everywhere.
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
        // Sanitize BEFORE clamp: Rust clamp propagates NaN and a hostile doc
        // value must never poison the uniform stream (panic/NaN-free law).
        let san = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let fov = san(self.cfg.fov, 60.0).clamp(30.0, 110.0);
        let fov_tan = (fov.to_radians() * 0.5).tan();
        EngineUniforms {
            shape: vec4(
                (self.cfg.steps.clamp(16, 120)) as f32,
                san(self.cfg.max_dist, 40.0).clamp(5.0, 200.0),
                self.cfg.cam as i32 as f32,
                san(self.cfg.cam_speed, 0.25).clamp(-8.0, 8.0),
            ),
            flow: vec4(
                san(self.cfg.cam_dist, 7.0).clamp(0.5, 60.0),
                san(self.cfg.cam_height, 2.2).clamp(-20.0, 40.0),
                fov_tan,
                if san(self.cfg.shadow, 1.0) >= 0.5 { 1.0 } else { 0.0 },
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // The base marcher. Kept deliberately modest (silent shader size
    // budget): one march loop, a 4-tap tetrahedral normal (BRANCHED NaN
    // guard — a flat field interior must fall back, never normalize(0)),
    // 3-tap AO, an optional 14-step soft shadow, one refraction branch.
    // Scenes live in preset subclasses of `scene_sdf`, never in here.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxRaymarch = set_type_default() do #(DrawVjFxRaymarch::script_shader(vm)){
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
        depth_write: false

        v_uv: varying(vec2f)

        // ---- clean-room SDF toolkit (scenes call these) ------------------
        sd_box: fn(p: vec3, b: vec3) -> float {
            let q = abs(p) - b
            return length(max(q, vec3(0.0, 0.0, 0.0)))
                + min(max(q.x, max(q.y, q.z)), 0.0)
        }

        // Floor-based domain repetition: safe for negative coords on every
        // backend (modf compiles to sign-preserving fmod on Metal/HLSL —
        // fract is floor-based everywhere, so use fract).
        rep1: fn(x: float, c: float) -> float {
            return (fract(x / c + 0.5) - 0.5) * c
        }

        rot2: fn(p: vec2, a: float) -> vec2 {
            let cs = cos(a)
            let sn = sin(a)
            return vec2(p.x * cs - p.y * sn, p.x * sn + p.y * cs)
        }

        smin: fn(a: float, b: float, k: float) -> float {
            let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0)
            return mix(b, a, h) - k * h * (1.0 - h)
        }

        // ---- THE SUBCLASS HOOK: the scene -------------------------------
        // (distance, material). material < 0 = glass (refracts input0).
        // Default: a floor and a grid of beat-breathing orbs — replaced by
        // every preset.
        scene_sdf: fn(p: vec3) -> vec2 {
            let floor_d = p.y + 1.4
            let q = vec3(self.rep1(p.x, 3.2), p.y, self.rep1(p.z, 3.2))
            let r = 0.65 + 0.22 * self.time_beat.w
            let ball = length(q) - r
            let d = self.smin(floor_d, ball, 0.6)
            let m = clamp(p.y * 0.3 + 0.4, 0.0, 1.0)
            return vec2(d, m)
        }

        // ---- palette hook: cosine palette, own constants -----------------
        fx_palette: fn(t: float) -> vec3 {
            let mid = (self.col_a.xyz + self.col_b.xyz) * 0.5
            let amp = (self.col_a.xyz - self.col_b.xyz) * 0.5
            let ph = vec3(0.0, 0.9, 1.8)
            return mid + amp * cos(vec3(6.2831853, 6.2831853, 6.2831853) * t + ph)
        }

        // Global deformation wrapper: the shared `twist` key bends EVERY
        // scene around Y; the distance is Lipschitz-compensated so the
        // march stays an underestimate.
        map: fn(p: vec3) -> vec2 {
            let tw = self.anim.w
            if abs(tw) > 0.0001 {
                let q = self.rot2(vec2(p.x, p.z), p.y * tw * 0.3)
                let dm = self.scene_sdf(vec3(q.x, p.y, q.y))
                return vec2(dm.x / (1.0 + abs(tw) * 0.5), dm.y)
            }
            return self.scene_sdf(p)
        }

        // Tetrahedral 4-tap normal. Flat interiors yield a zero gradient:
        // BRANCH to a fallback, never normalize a near-zero vector (the
        // NaN-normal law).
        nrm: fn(p: vec3) -> vec3 {
            let e = 0.0018
            let k0 = vec3(1.0, -1.0, -1.0)
            let k1 = vec3(-1.0, -1.0, 1.0)
            let k2 = vec3(-1.0, 1.0, -1.0)
            let k3 = vec3(1.0, 1.0, 1.0)
            let g = k0 * self.map(p + k0 * e).x
                + k1 * self.map(p + k1 * e).x
                + k2 * self.map(p + k2 * e).x
                + k3 * self.map(p + k3 * e).x
            let l = length(g)
            if l < 0.00001 {
                return vec3(0.0, 1.0, 0.0)
            }
            return g / l
        }

        // 3-tap ambient occlusion along the normal.
        occl: fn(p: vec3, n: vec3) -> float {
            let a = clamp(self.map(p + n * 0.12).x / 0.12, 0.0, 1.0)
            let b = clamp(self.map(p + n * 0.30).x / 0.30, 0.0, 1.0)
            let c = clamp(self.map(p + n * 0.65).x / 0.65, 0.0, 1.0)
            return clamp(a * 0.45 + b * 0.35 + c * 0.20, 0.0, 1.0)
        }

        // 14-step soft shadow march (flow.w gates it off for cheap presets).
        shad: fn(p: vec3, l: vec3) -> float {
            if self.flow.w < 0.5 {
                return 1.0
            }
            let mut res = 1.0
            let mut t = 0.08
            let mut i = 0.0
            loop {
                if i > 13.5 {
                    break
                }
                let d = self.map(p + l * t).x
                res = min(res, 8.0 * d / t)
                t = t + clamp(d, 0.03, 0.6)
                if res < 0.02 {
                    break
                }
                if t > 10.0 {
                    break
                }
                i = i + 1.0
            }
            return clamp(res, 0.0, 1.0)
        }

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

        pixel: fn() {
            let tm = self.time_beat.x
            let mode = self.shape.z
            let spd = self.shape.w
            let dist = self.flow.x
            let hgt = self.flow.y
            // -- camera program --------------------------------------------
            let mut eye = vec3(0.0, hgt, dist)
            let mut ta = vec3(0.0, hgt * 0.35, 0.0)
            if mode < 0.5 {
                let yaw = tm * spd
                eye = vec3(sin(yaw) * dist, hgt, cos(yaw) * dist)
            } else {
                if mode < 1.5 {
                    // FLY: endless forward down -z with a gentle weave.
                    // cam_dist is unused while flying, so it becomes the
                    // weave dial: amplitude = cam_dist * 0.17.
                    let wamp = dist * 0.17
                    let z = 0.0 - tm * spd
                    eye = vec3(sin(tm * 0.31) * wamp, hgt + sin(tm * 0.23) * 0.4, z)
                    ta = vec3(sin((tm + 2.2) * 0.31) * wamp, hgt, z - 5.0)
                } else {
                    // DOLLY: beat-bounced push-in.
                    let d2 = dist * (0.8 + 0.2 * cos(self.time_beat.y * spd))
                    eye = vec3(0.0, hgt, d2)
                }
            }
            let fwd = normalize(ta - eye)
            let right = normalize(cross(fwd, vec3(0.0, 1.0, 0.0)))
            let up = cross(right, fwd)
            let ft = self.flow.z
            let ndc = vec2(
                (self.v_uv.x - 0.5) * 2.0 * self.rm.x,
                (0.5 - self.v_uv.y) * 2.0
            )
            let rd = normalize(fwd + right * (ndc.x * ft) + up * (ndc.y * ft))

            // -- sphere trace ---------------------------------------------
            let steps = self.shape.x
            let maxd = self.shape.y
            let mut t = 0.02
            let mut mat = 0.0
            let mut hit = 0.0
            let mut i = 0.0
            loop {
                if i > 119.5 {
                    break
                }
                if i > steps - 0.5 {
                    break
                }
                let dm = self.map(eye + rd * t)
                if dm.x < 0.0016 * (1.0 + t) {
                    hit = 1.0
                    mat = dm.y
                    break
                }
                t = t + dm.x * 0.92
                if t > maxd {
                    break
                }
                i = i + 1.0
            }

            // -- background ------------------------------------------------
            let ldir = normalize(vec3(0.45, 0.65, 0.35))
            let horizon = 1.0 - clamp(abs(rd.y) * 2.2, 0.0, 1.0)
            let sunw = pow(clamp(dot(rd, ldir), 0.0, 1.0), 7.0)
            let bg = self.col_bg.xyz * (0.55 + 0.45 * horizon)
                + self.col_c.xyz * (sunw * 0.20 * self.fog.y)
            if hit < 0.5 {
                return vec4(bg, 1.0)
            }

            // -- shading ---------------------------------------------------
            let p = eye + rd * t
            let n = self.nrm(p)
            let mut col = vec3(0.0, 0.0, 0.0)
            if mat < -0.5 {
                // GLASS: one Snell bend, then sample input0 along the bent
                // ray. Total-internal-reflection BRANCHES to a mirror bounce
                // (never a mix guard).
                let eta = 1.0 / (1.30 + clamp(self.user.w, -0.25, 2.0))
                let cosi = clamp(0.0 - dot(rd, n), 0.0, 1.0)
                let kk = 1.0 - eta * eta * (1.0 - cosi * cosi)
                let mut rd2 = rd
                if kk < 0.0 {
                    rd2 = rd - n * (2.0 * dot(rd, n))
                } else {
                    rd2 = rd * eta + n * (eta * cosi - sqrt(kk))
                }
                let duv = vec2(rd2.x - rd.x, rd.y - rd2.y)
                let uv2 = clamp(
                    self.v_uv + duv * 0.55,
                    vec2(0.001, 0.001),
                    vec2(0.999, 0.999)
                )
                let texel = self.tex0.sample_as_bgra(uv2)
                let fres = pow(1.0 - cosi, 3.0)
                col = texel.xyz * (0.72 + 0.28 * self.time_beat.w)
                    + self.col_c.xyz * (fres * 0.55)
                    + self.fx_palette(0.5 + n.y * 0.3).xyz * 0.08
            } else {
                let dif = clamp(dot(n, ldir), 0.0, 1.0)
                let sh = self.shad(p + n * 0.03, ldir)
                let ao = self.occl(p, n)
                let hv = normalize(ldir - rd)
                let spec = pow(clamp(dot(n, hv), 0.0, 1.0), 24.0)
                let base = self.fx_palette(mat)
                let key = dif * sh * (0.85 + self.time_beat.w * 0.55)
                let amb = 0.16 + 0.10 * clamp(n.y, 0.0, 1.0)
                // CONTENT: the channel video projects onto the field, planar
                // by the dominant normal axis (floors xz, walls xy/zy),
                // folded into the albedo at low strength and lit like the
                // material. fog.z = the pre-gated `content` strength.
                let an = abs(n)
                let mut cuv = vec2(p.x, 0.0 - p.y)
                if an.y > max(an.x, an.z) {
                    cuv = vec2(p.x, p.z)
                } else { if an.x > an.z {
                    cuv = vec2(p.z, 0.0 - p.y)
                } }
                let tx = self.tex0.sample_as_bgra(fract(cuv * 0.09))
                let alb = base.mix(base * 0.35 + tx.xyz * 0.9, self.fog.z * 0.7)
                col = alb * ((amb + key) * ao) * self.fog.y
                    + self.col_c.xyz * (spec * sh * 0.35)
            }
            let fogf = exp(0.0 - t * self.fog.x)
            let rgb = col.mix(bg, 1.0 - fogf)
            return vec4(rgb, 1.0)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (the same 11 instance vec4s every fx shader
/// carries) + one raymarch extra: `rm` = (viewport aspect, 0, 0, 0), written
/// by the view's dispatch arm.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxRaymarch {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0/p1/p2 = free scene levers, p3 = glass IOR bend.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist — the global field twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (march steps, max dist, cam mode, cam speed).
    #[live(vec4(64.0, 40.0, 0.0, 0.25))]
    pub shape: Vec4f,
    /// (cam dist, cam height, tan(fov/2), shadow on/off).
    #[live(vec4(7.0, 2.2, 0.5773, 1.0))]
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
    /// (viewport aspect, unused, unused, unused).
    #[live(vec4(1.7777, 0.0, 0.0, 0.0))]
    pub rm: Vec4f,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raymarch_builds_one_fullscreen_quad() {
        let mut e = RaymarchEngine::new(RaymarchConfig::default());
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        assert_eq!(mesh.vertex_count(), 4);
        assert_eq!(mesh.triangle_count(), 2);
        // Corners must span the full clip square with screen-style uv
        // (uv.y = 0 at clip y = +1).
        let floats = super::super::mesh::VERT_FLOATS;
        for v in mesh.verts.chunks(floats) {
            assert_eq!(v[0].abs(), 1.0);
            assert_eq!(v[1].abs(), 1.0);
            let uv_y = v[9];
            assert_eq!(uv_y, if v[1] > 0.0 { 0.0 } else { 1.0 });
        }
    }

    #[test]
    fn raymarch_degenerate_params_stay_safe() {
        // Hostile doc values must clamp, not panic or emit NaN uniforms.
        let e = RaymarchEngine::new(RaymarchConfig {
            steps: 0,
            max_dist: f32::NAN,
            cam_speed: f32::INFINITY,
            cam_dist: -3.0,
            cam_height: f32::NAN,
            fov: 1000.0,
            shadow: f32::NAN,
            ..Default::default()
        });
        let u = e.uniforms();
        for v in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w]
        {
            assert!(v.is_finite(), "non-finite uniform {v}");
        }
        assert!(u.shape.x >= 16.0 && u.shape.x <= 120.0);
        assert!(u.flow.z > 0.0 && u.flow.z < 2.0, "fov tan out of range: {}", u.flow.z);
    }
}
