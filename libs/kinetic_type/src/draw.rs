//! B4, the glyph draw: `mod.draw.DrawKineticGlyph`, one shape's mesh drawn
//! once per record the animator wrote for it. The instance fields are the
//! animator's output record (the kernel writes this struct in place, via
//! `layout_of`); the look is Splash, replaced per kit.
//!
//! Vertex: `world = pos + rot * shear(scale * deform(p))`, normals turned
//! with the inverse scale (a mirrored copy, `scale.y < 0`, lights right).
//! Pixel: `look()` for letters and cubes, `floor()` for the floor plane.
//! On `self` the look reads: `face` (0 front, 1 back, 2 side, 3 bevel,
//! 4 cube, 5 floor), `nrm` (world normal), `wpos`, `lpos` (glyph-local),
//! `luv` (uv in the glyph's ink box), the record's `color`, `attr`, `info`
//! (t, word, line, index), the frame's `time beat phase pulse bar energy`,
//! the dials `p` (and each by its name, `self.swing()`), the palette
//! `col_a col_b col_c col_bg`, `self.eye()`, `self.content(uv)` (the
//! picture under the layer) and the stock finishes (`finish(mat, …)`,
//! `shade`, `env`, `spec`, `hue`, `key`, `rim`).

use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawKineticGlyph = mod.std.set_type_default() do #(DrawKineticGlyph::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        content_tex: texture_2d(float)

        backface_culling: false
        alpha_blend: true
        depth_write: true

        time: uniform(0.0)
        beat: uniform(0.0)
        phase: uniform(0.0)
        pulse: uniform(0.0)
        bar: uniform(0.0)
        energy: uniform(0.0)
        bpm: uniform(120.0)
        p: uniform(vec4(0.5, 0.5, 0.5, 0.5))
        // bass, mid, high, level (0..1)
        bands: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        col_a: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        col_b: uniform(vec4(0.3, 0.3, 0.3, 1.0))
        col_c: uniform(vec4(1.0, 0.5, 0.2, 1.0))
        col_bg: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        // x = the finish (0 matte 1 metal 2 neon 3 plastic 4 glass 5 holo),
        // y = 1 when content is bound, z = the text's width, w = its height
        k_misc: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        // xy = the target in pixels, z = the time the text last changed
        k_view: uniform(vec4(1920.0, 1080.0, 0.0, 0.0))

        face: varying(float)
        nrm: varying(vec3f)
        wpos: varying(vec3f)
        lpos: varying(vec3f)
        luv: varying(vec2f)

        qrot: fn(q: vec4, v: vec3) -> vec3 {
            let t = vec3(q.y * v.z - q.z * v.y, q.z * v.x - q.x * v.z, q.x * v.y - q.y * v.x) * 2.0
            return v + t * q.w + vec3(q.y * t.z - q.z * t.y, q.z * t.x - q.x * t.z, q.x * t.y - q.y * t.x)
        }

        // A vertex hook: the glyph-local position bent before it is placed
        // (a dome, a squash, a twist). Identity by default.
        deform: fn(p: vec3) -> vec3 {
            return p
        }

        vertex: fn() {
            let lp = self.deform(self.geom.geom_pos)
            let sp = lp * self.scale
            let sh = vec3(sp.x + self.shear.x * sp.y, sp.y + self.shear.y * sp.x, sp.z)
            let wp = self.qrot(self.rot, sh) + self.pos
            let s = self.scale
            let inv = vec3(sign(s.x) / max(abs(s.x), 0.0001), sign(s.y) / max(abs(s.y), 0.0001), sign(s.z) / max(abs(s.z), 0.0001))
            self.nrm = self.qrot(self.rot, self.geom.geom_normal * inv)
            self.wpos = wp
            self.lpos = lp
            self.face = self.geom.geom_pad
            self.luv = self.geom.geom_uv
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * vec4(wp.x, wp.y, wp.z, 1.0))
        }

        // ---- signals and helpers for looks ----
        eye: fn() -> vec3 {
            let c = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            return c.xyz / max(c.w, 0.0001)
        }
        n: fn() -> vec3 {
            return self.nrm / max(length(self.nrm), 0.001)
        }
        vd: fn() -> vec3 {
            return normalize(self.eye() - self.wpos)
        }
        key: fn() -> float {
            return max(dot(self.n(), vec3(0.3, 0.55, 0.78)), 0.0)
        }
        rim: fn() -> float {
            return pow(1.0 - abs(self.n().z), 2.0)
        }
        // 1 on a face that looks out of the letter (front cap, cube).
        cap: fn() -> float {
            return 1.0 - step(0.5, self.face) + step(3.5, self.face) * (1.0 - step(4.5, self.face))
        }
        content: fn(uv: vec2) -> vec4 {
            if self.k_misc.y < 0.5 {
                return self.col_bg
            }
            return self.content_tex.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }
        // The frame uv of this pixel (0,0 top left).
        screen_uv: fn() -> vec2 {
            let c = self.draw_pass.camera_projection * (self.draw_pass.camera_view * vec4(self.wpos.x, self.wpos.y, self.wpos.z, 1.0))
            let q = c.xy / max(c.w, 0.0001)
            return vec2(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5)
        }
        // Exponential distance fog toward col_bg.
        fog: fn(c: vec3, density: float) -> vec3 {
            let d = length(self.eye() - self.wpos)
            return self.col_bg.xyz.mix(c, exp(0.0 - d * density))
        }
        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        // ---- the stock finishes ----
        shade: fn(nn: vec3) -> float {
            let key = max(dot(nn, vec3(0.36, 0.48, 0.80)), 0.0)
            let fill = max(dot(nn, vec3(-0.65, -0.25, 0.72)), 0.0)
            return 0.30 + 0.70 * key + 0.25 * fill
        }
        env: fn(r: vec3) -> vec3 {
            let up = clamp(r.y, -1.0, 1.0)
            let sky = vec3(0.95, 0.97, 1.0).mix(vec3(0.22, 0.28, 0.40), sqrt(max(up, 0.0)))
            let ground = vec3(0.34, 0.30, 0.26).mix(vec3(0.02, 0.02, 0.025), sqrt(max(0.0 - up, 0.0)))
            let mut e = ground
            if up > 0.0 {
                e = sky
            }
            return e + vec3(1.0, 0.96, 0.88) * exp(0.0 - abs(up) * 16.0)
        }
        spec: fn(nn: vec3, vd: vec3, power: float) -> float {
            let h = normalize(vec3(0.36, 0.48, 0.80) + vd)
            return pow(max(dot(nn, h), 0.0), power)
        }
        hue: fn(h: float) -> vec3 {
            let a = h * 6.2831853
            return vec3(0.5 + 0.5 * cos(a), 0.5 + 0.5 * cos(a - 2.0944), 0.5 + 0.5 * cos(a - 4.1888))
        }
        // mat: 0 matte, 1 metal, 2 neon, 3 plastic, 4 glass, 5 holo.
        finish: fn(mat: float, base: vec3, cap: float, nn: vec3, vd: vec3, t: float, uv: vec2) -> vec3 {
            let sh = self.shade(nn)
            let fres = pow(1.0 - clamp(abs(dot(nn, vd)), 0.0, 1.0), 3.0)
            let r = nn * (2.0 * dot(nn, vd)) - vd
            if mat < 0.5 {
                return base * sh
            }
            if mat < 1.5 {
                let rr = vec3(r.x, r.y + (0.5 - uv.y) * 0.9, r.z)
                return base * self.env(rr) * 1.1 + vec3(1.0, 1.0, 1.0) * self.spec(nn, vd, 90.0) * 1.3
            }
            if mat < 2.5 {
                let glow = 0.18 + 1.25 * cap * (1.0 + self.pulse * 0.4)
                return base * glow + self.col_c.xyz * fres * 1.6
            }
            if mat < 3.5 {
                return base * sh + vec3(1.0, 1.0, 1.0) * self.spec(nn, vd, 20.0) * 0.45
            }
            if mat < 4.5 {
                let mut behind = self.col_bg.xyz * 1.6 + self.env(r) * 0.10
                if self.k_misc.y > 0.5 {
                    behind = self.content(self.screen_uv() + nn.xy * 0.07).xyz
                }
                let body = behind * (0.55 + 0.35 * cap) + base * 0.22
                return body + self.col_c.xyz * fres * 1.3 + vec3(1.0, 1.0, 1.0) * self.spec(nn, vd, 60.0) * 0.8
            }
            let h = fract(abs(dot(nn, vd)) * 1.4 + self.bar + t * 0.35)
            return self.hue(h) * (0.45 + 0.75 * sh) + vec3(1.0, 1.0, 1.0) * self.spec(nn, vd, 40.0) * 0.6
        }

        // THE LOOK (a kit replaces it): caps in the record's colour, walls
        // col_b, bevel col_c, sung glyphs col_c, in the kit's finish.
        look: fn() -> vec4 {
            let nn = self.n()
            let cap = self.cap()
            let mut base = self.color.xyz
            if cap < 0.5 {
                base = self.col_b.xyz * 0.55
            }
            if abs(self.face - 3.0) < 0.5 {
                base = self.col_c.xyz * 0.85
            }
            if self.attr.x > 0.5 {
                base = self.col_c.xyz
            }
            let uv = vec2(self.luv.x, 1.0 - self.luv.y)
            let lit = self.finish(self.k_misc.x, base, cap, nn, self.vd(), self.info.x, uv)
            return vec4(lit * (1.0 + self.pulse * 0.25), self.color.w)
        }

        // THE FLOOR (a kit replaces it): col_bg with col_b grid lines and a
        // glossy streak, faded at the edge; alpha is its cover over the
        // reflections.
        floor: fn() -> vec4 {
            let cell = 1.0
            let p = self.wpos.xz
            let gx = fract(p.x / cell)
            let gz = fract(p.y / cell)
            let d = min(min(gx, 1.0 - gx), min(gz, 1.0 - gz))
            let line = 1.0 - smoothstep(0.0, 0.04, d)
            let vd = self.vd()
            let r = vec3(0.0, 1.0, 0.0) * (2.0 * vd.y) - vd
            let streak = pow(max(dot(r, normalize(vec3(0.0, 0.22, -1.0))), 0.0), 18.0)
            let rgb = self.col_bg.xyz * 1.4 + self.col_b.xyz * line * (0.35 + 0.6 * self.pulse) + vec3(1.0, 1.0, 1.0) * streak * 0.3
            let edge = 1.0 - smoothstep(0.30, 0.5, max(abs(self.luv.x - 0.5), abs(self.luv.y - 0.5)))
            return vec4(rgb, 0.55 * edge)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }

        pixel: fn() {
            let mut c = vec4(0.0, 0.0, 0.0, 0.0)
            if self.face > 4.5 {
                c = self.floor()
            } else {
                c = self.look()
            }
            let a = clamp(c.w, 0.0, 1.0)
            return vec4(c.xyz * a, a)
        }
    }

    // The backdrop: a full-frame quad behind everything (drawn first, at
    // the far plane, no depth write). `backdrop()` reads `self.pos` (0,0
    // top left) and the same signals as the look.
    mod.draw.DrawKineticBackdrop = mod.std.set_type_default() do #(DrawKineticBackdrop::script_shader(vm)){
        ..mod.draw.DrawQuad
        content_tex: texture_2d(float)
        depth_write: false
        time: uniform(0.0)
        beat: uniform(0.0)
        phase: uniform(0.0)
        pulse: uniform(0.0)
        bar: uniform(0.0)
        energy: uniform(0.0)
        bpm: uniform(120.0)
        p: uniform(vec4(0.5, 0.5, 0.5, 0.5))
        // bass, mid, high, level (0..1)
        bands: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        col_a: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        col_b: uniform(vec4(0.3, 0.3, 0.3, 1.0))
        col_c: uniform(vec4(1.0, 0.5, 0.2, 1.0))
        col_bg: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        k_misc: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        k_view: uniform(vec4(1920.0, 1080.0, 0.0, 0.0))
        vertex: fn() {
            self.pos = self.geom.pos
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.99999, 1.0)
        }
        content: fn(uv: vec2) -> vec4 {
            if self.k_misc.y < 0.5 {
                return self.col_bg
            }
            return self.content_tex.sample_as_bgra(clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0)))
        }
        backdrop: fn() -> vec4 {
            return vec4(self.col_bg.xyz, 1.0)
        }
        pixel: fn() {
            let c = self.backdrop()
            let a = clamp(c.w, 0.0, 1.0)
            return vec4(c.xyz * a, a)
        }
    }
}

/// The glyph draw. Every field after the base is an instance field: the
/// record the animator kernel writes (reflected with `layout_of`, never
/// mirrored by hand).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKineticGlyph {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub pos: Vec3f,
    #[live(vec4(0.0, 0.0, 0.0, 1.0))]
    pub rot: Vec4f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub scale: Vec3f,
    #[live]
    pub shear: Vec2f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub color: Vec4f,
    #[live]
    pub attr: Vec4f,
    #[live]
    pub info: Vec4f,
    #[live]
    pub shape: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKineticBackdrop {
    #[deref]
    pub draw_super: DrawQuad,
}
