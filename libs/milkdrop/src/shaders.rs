//! The draw shaders of a MilkDrop frame.
//!
//! Every preset-dependent stage is a base shader here whose preset-specific
//! part is ONE method a subclass replaces:
//!
//! | shader | stage | replaced method |
//! |---|---|---|
//! | [`DrawMilkMesh`] | per-vertex equations → warped UV per grid vertex (float target) | `milk_vertex` |
//! | [`DrawMilkWarp`] | previous frame through the warp mesh + warp shader → feedback buffer | `milk_body` (+ `fn_*` helpers) |
//! | [`DrawMilkComp`] | feedback buffer → picture (composite shader or the MilkDrop 1 default) | `milk_body` (+ `fn_*`) |
//! | [`DrawMilkWave`] | custom wave segments, per-point equations in the vertex stage | `milk_point` |
//!
//! Fixed stages: [`DrawMilkBlur`] (separable gaussian), [`DrawMilkShape`]
//! (custom shape instances), [`DrawMilkLine`] (basic wave, motion vectors,
//! borders), [`DrawMilkBlit`] (display + preset transition blend).
//!
//! `DrawMilkBase` holds what every preset stage reads: the uniform block
//! (time, audio, q1..q32, roam, aspect, per-frame motion values) and the
//! texture set in a fixed order, with the MilkDrop helper functions
//! (`get_main`, `get_blur1..3`, `noisevol_lq/hq`, `texsize_*`, the warp UV
//! formula). Texture slots are bound by declaration index; see
//! [`TEX_SLOTS`].

use makepad_widgets::*;

/// Texture slot order of every shader derived from `DrawMilkBase`.
pub const TEX_MAIN: usize = 0;
pub const TEX_MESH: usize = 1;
pub const TEX_BLUR1: usize = 2;
pub const TEX_NOISE_LQ: usize = 5;
pub const TEX_NOISE_LQ_LITE: usize = 6;
pub const TEX_NOISE_MQ: usize = 7;
pub const TEX_NOISE_HQ: usize = 8;
pub const TEX_NOISEVOL_LQ: usize = 9;
pub const TEX_NOISEVOL_HQ: usize = 10;
pub const TEX_IMG0: usize = 11;
/// Slots in use (≤ the platform's 17 per draw call).
pub const TEX_SLOTS: usize = 15;

/// Side of the volume noise textures, stored as an 8×4 atlas of slices.
pub const NOISEVOL_SIZE: usize = 32;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawMilkBase = mod.std.set_type_default() do #(DrawMilkBase::script_shader(vm)){
        ..mod.draw.DrawQuad

        tex_main: texture_2d(float)
        tex_mesh: texture_2d(float)
        tex_blur1: texture_2d(float)
        tex_blur2: texture_2d(float)
        tex_blur3: texture_2d(float)
        tex_noise_lq: texture_2d(float)
        tex_noise_lq_lite: texture_2d(float)
        tex_noise_mq: texture_2d(float)
        tex_noise_hq: texture_2d(float)
        tex_noisevol_lq: texture_2d(float)
        tex_noisevol_hq: texture_2d(float)
        tex_img0: texture_2d(float)
        tex_img1: texture_2d(float)
        tex_img2: texture_2d(float)
        tex_img3: texture_2d(float)

        // (time, fps, frame, progress)
        u_time: uniform(vec4(0.0, 60.0, 0.0, 0.0))
        // (bass, mid, treb, vol) and the attenuated set
        u_audio: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_audio_att: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // (aspectx, aspecty, 1/aspectx, 1/aspecty)
        u_aspect: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_q0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_q7: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_rand_frame: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_rand_preset: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_roam_cos: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_roam_sin: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_slow_roam_cos: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_slow_roam_sin: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // (blur1, blur2, blur3, _) ranges
        u_blur_min: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_blur_max: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // Per-frame motion: (zoom, zoomexp, rot, warp), (cx, cy, dx, dy),
        // (sx, sy, decay, wrap)
        u_pf0: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        u_pf1: uniform(vec4(0.5, 0.5, 0.0, 0.0))
        u_pf2: uniform(vec4(1.0, 1.0, 0.98, 1.0))
        // Warp animation: (warp time, 1/warp scale, _, _) and the four
        // MilkDrop warp frequencies
        u_warp_t: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        u_warp_f: uniform(vec4(11.68, 8.77, 10.54, 11.49))
        // Mesh grid: (cells x, cells y, vertices x, vertices y)
        u_mesh: uniform(vec4(48.0, 36.0, 49.0, 37.0))
        // (pixels x, pixels y, _, _)
        u_pixels: uniform(vec4(1280.0, 720.0, 0.0, 0.0))

        texsize_main: fn() -> vec4 {
            let s = self.tex_main.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_blur1: fn() -> vec4 {
            let s = self.tex_blur1.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_blur2: fn() -> vec4 {
            let s = self.tex_blur2.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_blur3: fn() -> vec4 {
            let s = self.tex_blur3.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_img0: fn() -> vec4 {
            let s = self.tex_img0.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_img1: fn() -> vec4 {
            let s = self.tex_img1.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_img2: fn() -> vec4 {
            let s = self.tex_img2.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }
        texsize_of_img3: fn() -> vec4 {
            let s = self.tex_img3.size()
            return vec4(s.x, s.y, 1.0 / max(s.x, 1.0), 1.0 / max(s.y, 1.0))
        }

        // GetPixel / GetMain: the feedback buffer, bilinear + wrap.
        get_main: fn(uv: vec2) -> vec3 {
            return self.tex_main.sample_repeat(uv).xyz
        }

        // GetBlurN: the blur level, held to the preset's [min, max] range.
        get_blur1: fn(uv: vec2) -> vec3 {
            let c = self.tex_blur1.sample(uv).xyz
            return clamp(c, vec3(self.u_blur_min.x, self.u_blur_min.x, self.u_blur_min.x), vec3(self.u_blur_max.x, self.u_blur_max.x, self.u_blur_max.x))
        }
        get_blur2: fn(uv: vec2) -> vec3 {
            let c = self.tex_blur2.sample(uv).xyz
            return clamp(c, vec3(self.u_blur_min.y, self.u_blur_min.y, self.u_blur_min.y), vec3(self.u_blur_max.y, self.u_blur_max.y, self.u_blur_max.y))
        }
        get_blur3: fn(uv: vec2) -> vec3 {
            let c = self.tex_blur3.sample(uv).xyz
            return clamp(c, vec3(self.u_blur_min.z, self.u_blur_min.z, self.u_blur_min.z), vec3(self.u_blur_max.z, self.u_blur_max.z, self.u_blur_max.z))
        }

        // tex3D on the volume noises: the 32³ volume is stored as 32 slices
        // in an 8×4 atlas; trilinear by hand from nearest texel fetches
        // (wrapping in all three axes, like the D3D volume sampler).
        vol_lq_texel: fn(c: vec3) -> vec4 {
            let w = c - floor(c / 32.0) * 32.0
            let z = floor(w.z)
            let tile = vec2(z - floor(z / 8.0) * 8.0, floor(z / 8.0))
            let px = tile * 32.0 + floor(w.xy) + vec2(0.5, 0.5)
            return self.tex_noisevol_lq.sample_nearest(px / vec2(256.0, 128.0))
        }
        vol_hq_texel: fn(c: vec3) -> vec4 {
            let w = c - floor(c / 32.0) * 32.0
            let z = floor(w.z)
            let tile = vec2(z - floor(z / 8.0) * 8.0, floor(z / 8.0))
            let px = tile * 32.0 + floor(w.xy) + vec2(0.5, 0.5)
            return self.tex_noisevol_hq.sample_nearest(px / vec2(256.0, 128.0))
        }
        noisevol_lq: fn(p: vec3) -> vec4 {
            let q = p * 32.0 - vec3(0.5, 0.5, 0.5)
            let i = floor(q)
            let f = q - i
            let c00 = mix(self.vol_lq_texel(i), self.vol_lq_texel(i + vec3(1.0, 0.0, 0.0)), f.x)
            let c10 = mix(self.vol_lq_texel(i + vec3(0.0, 1.0, 0.0)), self.vol_lq_texel(i + vec3(1.0, 1.0, 0.0)), f.x)
            let c01 = mix(self.vol_lq_texel(i + vec3(0.0, 0.0, 1.0)), self.vol_lq_texel(i + vec3(1.0, 0.0, 1.0)), f.x)
            let c11 = mix(self.vol_lq_texel(i + vec3(0.0, 1.0, 1.0)), self.vol_lq_texel(i + vec3(1.0, 1.0, 1.0)), f.x)
            return mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z)
        }
        noisevol_hq: fn(p: vec3) -> vec4 {
            let q = p * 32.0 - vec3(0.5, 0.5, 0.5)
            let i = floor(q)
            let f = q - i
            let c00 = mix(self.vol_hq_texel(i), self.vol_hq_texel(i + vec3(1.0, 0.0, 0.0)), f.x)
            let c10 = mix(self.vol_hq_texel(i + vec3(0.0, 1.0, 0.0)), self.vol_hq_texel(i + vec3(1.0, 1.0, 0.0)), f.x)
            let c01 = mix(self.vol_hq_texel(i + vec3(0.0, 0.0, 1.0)), self.vol_hq_texel(i + vec3(1.0, 0.0, 1.0)), f.x)
            let c11 = mix(self.vol_hq_texel(i + vec3(0.0, 1.0, 1.0)), self.vol_hq_texel(i + vec3(1.0, 1.0, 1.0)), f.x)
            return mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z)
        }

        // EEL semantics for equations evaluated on the GPU (eel_gpu.rs):
        // division by zero is 0, `%` is integer modulo, `^` of a negative
        // base needs an integer exponent, comparisons use 1e-5.
        eel_div: fn(a: float, b: float) -> float {
            if b == 0.0 { return 0.0 }
            return a / b
        }
        eel_mod: fn(a: float, b: float) -> float {
            let bi = trunc(b)
            if bi == 0.0 { return 0.0 }
            let ai = trunc(a)
            return ai - bi * trunc(ai / bi)
        }
        eel_fmod: fn(a: float, b: float) -> float {
            if b == 0.0 { return 0.0 }
            return a - b * trunc(a / b)
        }
        eel_pow: fn(a: float, b: float) -> float {
            if a >= 0.0 { return pow(a, b) }
            if fract(b) != 0.0 { return 0.0 }
            let r = pow(-a, b)
            let odd = abs(b) - floor(abs(b) / 2.0) * 2.0
            if odd >= 0.5 { return -r }
            return r
        }
        eel_eq: fn(a: float, b: float) -> float {
            if abs(a - b) < 0.00001 { return 1.0 }
            return 0.0
        }
        eel_lt: fn(a: float, b: float) -> float {
            if a < b { return 1.0 }
            return 0.0
        }
        eel_not: fn(a: float) -> float {
            if abs(a) < 0.00001 { return 1.0 }
            return 0.0
        }
        eel_band: fn(a: float, b: float) -> float {
            if abs(a) >= 0.00001 && abs(b) >= 0.00001 { return 1.0 }
            return 0.0
        }
        eel_bor: fn(a: float, b: float) -> float {
            if abs(a) >= 0.00001 || abs(b) >= 0.00001 { return 1.0 }
            return 0.0
        }
        // EEL's `&` and `|`: on the values truncated to integers.
        eel_bitand: fn(a: float, b: float) -> float {
            return float(int(a) & int(b))
        }
        eel_bitor: fn(a: float, b: float) -> float {
            return float(int(a) | int(b))
        }
        eel_sigmoid: fn(a: float, b: float) -> float {
            let t = 1.0 + exp(-a * b)
            if abs(t) > 0.00001 { return 1.0 / t }
            return 0.0
        }
        eel_log: fn(a: float) -> float {
            if a > 0.0 { return log(a) }
            return 0.0
        }
        eel_log10: fn(a: float) -> float {
            if a > 0.0 { return log(a) * 0.4342944819 }
            return 0.0
        }
        eel_invsqrt: fn(a: float) -> float {
            let s = sqrt(abs(a))
            if s > 0.0 { return 1.0 / s }
            return 0.0
        }
        eel_asin: fn(a: float) -> float {
            return asin(clamp(a, -1.0, 1.0))
        }
        eel_acos: fn(a: float) -> float {
            return acos(clamp(a, -1.0, 1.0))
        }

        // A deterministic GPU random number in [0, 1) for `rand()` in
        // per-vertex / per-point equations evaluated on the GPU.
        gpu_rand: fn(seed: vec2) -> float {
            return fract(sin(dot(seed, vec2(12.9898, 78.233)) + self.u_time.z * 0.618) * 43758.5453)
        }

        // Screen position (0..1, top-left) → MilkDrop's centred vertex
        // coordinates, y up: (fx, fy).
        milk_xy: fn(p: vec2) -> vec2 {
            return vec2(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0)
        }
        milk_rad: fn(f: vec2) -> float {
            return sqrt(f.x * f.x * self.u_aspect.x * self.u_aspect.x + f.y * f.y * self.u_aspect.y * self.u_aspect.y)
        }
        milk_ang: fn(f: vec2) -> float {
            return atan2(f.y * self.u_aspect.y, f.x * self.u_aspect.x)
        }

        // MilkDrop's per-vertex UV: zoom (with the radial zoom exponent),
        // stretch, the four-sinusoid warp, rotation, translation, aspect.
        warp_uv: fn(f: vec2, rad: float, m0: vec4, m1: vec4, m2: vec4) -> vec2 {
            // m0 = (zoom, zoomexp, rot, warp), m1 = (cx, cy, dx, dy),
            // m2 = (sx, sy, _, _)
            let zoom2 = pow(max(m0.x, 0.0001), pow(max(m0.y, 0.0001), rad * 2.0 - 1.0))
            let inv = 1.0 / max(abs(zoom2), 0.0001) * sign(zoom2 + 0.0000001)
            var u = f.x * self.u_aspect.x * 0.5 * inv + 0.5
            var v = -f.y * self.u_aspect.y * 0.5 * inv + 0.5
            // A stretch of exactly 0 would divide by zero; keep its sign.
            let sx = max(abs(m2.x), 0.0001) * sign(m2.x + 0.0000001)
            let sy = max(abs(m2.y), 0.0001) * sign(m2.y + 0.0000001)
            u = (u - m1.x) / sx + m1.x
            v = (v - m1.y) / sy + m1.y
            let wt = self.u_warp_t.x
            let ws = self.u_warp_t.y
            let wf = self.u_warp_f
            let w = m0.w * 0.0035
            u = u + w * sin(wt * 0.333 + ws * (f.x * wf.x - f.y * wf.w))
            v = v + w * cos(wt * 0.375 - ws * (f.x * wf.z + f.y * wf.y))
            u = u + w * cos(wt * 0.753 - ws * (f.x * wf.y - f.y * wf.z))
            v = v + w * sin(wt * 0.825 + ws * (f.x * wf.x + f.y * wf.w))
            let u2 = u - m1.x
            let v2 = v - m1.y
            let cr = cos(m0.z)
            let sr = sin(m0.z)
            u = u2 * cr - v2 * sr + m1.x
            v = u2 * sr + v2 * cr + m1.y
            u = u - m1.z
            v = v - m1.w
            u = (u - 0.5) * self.u_aspect.z + 0.5
            v = (v - 0.5) * self.u_aspect.w + 0.5
            return vec2(u, v)
        }
    }

    // Per-vertex equations, one grid vertex per target texel. The target is
    // (cells + 1)² texels of RGBA32F; .xy is the warped UV.
    mod.draw.DrawMilkMesh = mod.std.set_type_default() do #(DrawMilkMesh::script_shader(vm)){
        ..mod.draw.DrawMilkBase
        color_format: @Rgba32F

        // The preset's per-vertex code replaces this; the default is the
        // per-frame motion unchanged at every vertex.
        milk_vertex: fn(f: vec2, x: float, y: float, rad: float, ang: float) -> vec2 {
            return self.warp_uv(f, rad, self.u_pf0, self.u_pf1, self.u_pf2)
        }

        pixel: fn() {
            let g = floor(self.pos * self.u_mesh.zw)
            let f = vec2(g.x / self.u_mesh.x * 2.0 - 1.0, 1.0 - g.y / self.u_mesh.y * 2.0)
            let rad = self.milk_rad(f)
            let ang = self.milk_ang(f)
            let x = f.x * 0.5 * self.u_aspect.x + 0.5
            let y = -f.y * 0.5 * self.u_aspect.y + 0.5
            let uv = self.milk_vertex(f, x, y, rad, ang)
            return vec4(uv.x, uv.y, 0.0, 1.0)
        }
    }

    // The warp pass: previous frame through the mesh into the feedback
    // buffer. The mesh texture holds one warped UV per grid vertex;
    // between vertices it is interpolated bilinearly (MilkDrop draws the
    // mesh as triangles; a dense grid makes the difference invisible).
    mod.draw.DrawMilkWarp = mod.std.set_type_default() do #(DrawMilkWarp::script_shader(vm)){
        ..mod.draw.DrawMilkBase
        alpha_blend: false

        mesh_texel: fn(i: vec2) -> vec2 {
            return self.tex_mesh.sample_nearest((i + vec2(0.5, 0.5)) / self.u_mesh.zw).xy
        }
        mesh_uv: fn(p: vec2) -> vec2 {
            let g = p * self.u_mesh.xy
            let i = min(floor(g), self.u_mesh.xy - vec2(1.0, 1.0))
            let t = g - i
            let a = self.mesh_texel(i)
            let b = self.mesh_texel(i + vec2(1.0, 0.0))
            let c = self.mesh_texel(i + vec2(0.0, 1.0))
            let d = self.mesh_texel(i + vec2(1.0, 1.0))
            return mix(mix(a, b, t.x), mix(c, d, t.x), t.y)
        }

        // MilkDrop 1 warp: the previous frame, darkened by decay, sampled
        // wrapped or clamped (`wrap`). A preset's warp shader replaces it.
        milk_body: fn(uv_in: vec2, uv_orig: vec2, rad: float, ang: float) -> vec3 {
            var c = self.tex_main.sample(uv_in).xyz
            if self.u_pf2.w > 0.5 {
                c = self.tex_main.sample_repeat(uv_in).xyz
            }
            return c * self.u_pf2.z
        }

        pixel: fn() {
            let uv_orig = self.pos
            let uv = self.mesh_uv(self.pos)
            let f = self.milk_xy(uv_orig)
            let c = self.milk_body(uv, uv_orig, self.milk_rad(f), self.milk_ang(f))
            return vec4(clamp(c, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), 1.0)
        }
    }

    // The composite: feedback buffer to picture.
    mod.draw.DrawMilkComp = mod.std.set_type_default() do #(DrawMilkComp::script_shader(vm)){
        ..mod.draw.DrawMilkBase
        alpha_blend: false

        // (echo zoom, echo alpha, echo orientation, gamma)
        u_comp0: uniform(vec4(1.0, 0.0, 0.0, 1.0))
        // (brighten, darken, solarize, invert)
        u_comp1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // (darken center, _, _, _)
        u_comp2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // hue_shader corner colours: top-left, top-right, bottom-left,
        // bottom-right (rgb)
        u_hue0: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        u_hue1: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        u_hue2: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        u_hue3: uniform(vec4(1.0, 1.0, 1.0, 0.0))

        // MilkDrop 1 composite: video echo, gamma, then the brighten /
        // darken / solarize / invert filters. A preset's comp shader
        // replaces it.
        milk_body: fn(uv_in: vec2, uv_orig: vec2, rad: float, ang: float, hue_shader: vec3) -> vec3 {
            var c = self.tex_main.sample(uv_in).xyz
            if self.u_comp0.y > 0.001 {
                var e = (uv_in - vec2(0.5, 0.5)) / max(self.u_comp0.x, 0.0001)
                let o = self.u_comp0.z
                if (o > 0.5 && o < 1.5) || o > 2.5 { e.x = -e.x }
                if o > 1.5 { e.y = -e.y }
                let echo = self.tex_main.sample(e + vec2(0.5, 0.5)).xyz
                c = mix(c, echo, self.u_comp0.y)
            }
            c = c * self.u_comp0.w
            if self.u_comp2.x > 0.5 {
                c = c * (0.7 + 0.3 * clamp(rad * 1.4, 0.0, 1.0))
            }
            if self.u_comp1.x > 0.5 { c = sqrt(clamp(c, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))) }
            if self.u_comp1.y > 0.5 { c = c * c }
            if self.u_comp1.z > 0.5 { c = c * (vec3(1.0, 1.0, 1.0) - c) * 4.0 }
            if self.u_comp1.w > 0.5 { c = vec3(1.0, 1.0, 1.0) - c }
            return c
        }

        pixel: fn() {
            let uv = self.pos
            let f = self.milk_xy(uv)
            let top = mix(self.u_hue0.xyz, self.u_hue1.xyz, uv.x)
            let bottom = mix(self.u_hue2.xyz, self.u_hue3.xyz, uv.x)
            let hue = mix(top, bottom, uv.y)
            let c = self.milk_body(uv, uv, self.milk_rad(f), self.milk_ang(f), hue)
            return vec4(clamp(c, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), 1.0)
        }
    }

    // Separable gaussian: 9 taps along u_dir (in texels of the source).
    mod.draw.DrawMilkBlur = mod.std.set_type_default() do #(DrawMilkBlur::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        tex_src: texture_2d(float)
        // (dir x, dir y, edge darken, _) in source texels
        u_dir: uniform(vec4(1.0, 0.0, 0.0, 0.0))

        tap: fn(uv: vec2) -> vec3 {
            return self.tex_src.sample(uv).xyz
        }

        pixel: fn() {
            let s = self.tex_src.size()
            let d = self.u_dir.xy / max(s, vec2(1.0, 1.0))
            let uv = self.pos
            var c = self.tap(uv) * 0.2270270
            c = c + (self.tap(uv + d * 1.3846154) + self.tap(uv - d * 1.3846154)) * 0.3162162
            c = c + (self.tap(uv + d * 3.2307692) + self.tap(uv - d * 3.2307692)) * 0.0702703
            // Blur 1's edge darkening: MilkDrop fades the blur toward the
            // borders so feedback through it does not smear the edge in.
            if self.u_dir.z > 0.0 {
                let e = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y))
                c = c * mix(1.0, clamp(e * 12.0, 0.0, 1.0), self.u_dir.z)
            }
            return vec4(c, 1.0)
        }
    }

    // Wave line segments: custom waves (per-point equations evaluated here
    // when they run on the GPU), the basic wave and motion vectors (points
    // computed on the CPU). One instance per segment; the vertex stage
    // places both ends in wave space (0..1, y up) and extrudes the quad.
    mod.draw.DrawMilkWave = mod.std.set_type_default() do #(DrawMilkWave::script_shader(vm)){
        ..mod.draw.DrawMilkBase
        v_color: varying(vec4f)

        // The custom wave's per-frame colour and t1..t8.
        u_wave_rgba: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_wave_t0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_wave_t1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // (evaluate per-point code here, half width px, _, additive)
        u_wave_mode: uniform(vec4(0.0, 0.75, 0.0, 0.0))

        // CPU points: the instance carries the position in .xy. A custom
        // wave evaluated on the GPU replaces both methods.
        milk_point_xy: fn(sample_in: float, value1_in: float, value2_in: float) -> vec2 {
            return vec2(value1_in, value2_in)
        }
        milk_point_rgba: fn(sample_in: float, value1_in: float, value2_in: float) -> vec4 {
            return self.u_wave_rgba
        }

        vertex: fn() {
            let pa = self.milk_point_xy(self.a.x, self.a.y, self.a.z)
            let pb = self.milk_point_xy(self.b.x, self.b.y, self.b.z)
            let size = self.rect_size
            let sa = vec2(pa.x, 1.0 - pa.y) * size
            let sb = vec2(pb.x, 1.0 - pb.y) * size
            var d = sb - sa
            let len = length(d)
            if len < 0.0001 {
                d = vec2(1.0, 0.0)
            } else {
                d = d / len
            }
            let n = vec2(-d.y, d.x)
            let w = self.u_wave_mode.y
            let end = self.geom.pos.x
            let side = self.geom.pos.y * 2.0 - 1.0
            let p = mix(sa, sb, end) + d * (end * 2.0 - 1.0) * w + n * side * w
            var c = mix(self.ca, self.cb, end)
            if self.u_wave_mode.x > 0.5 {
                if end < 0.5 {
                    c = self.milk_point_rgba(self.a.x, self.a.y, self.a.z)
                } else {
                    c = self.milk_point_rgba(self.b.x, self.b.y, self.b.z)
                }
            }
            self.v_color = c
            self.pos = vec2(end, side)
            self.world = self.draw_list.view_transform * vec4(p.x + self.rect_pos.x, p.y + self.rect_pos.y, self.draw_depth + self.draw_call.zbias, 1.0)
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        pixel: fn() {
            let c = self.v_color
            let edge = 1.0 - smoothstep(0.55, 1.0, abs(self.pos.y))
            let a = clamp(c.w, 0.0, 1.0) * edge
            let rgb = clamp(c.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)) * a
            if self.u_wave_mode.w > 0.5 {
                return vec4(rgb, 0.0)
            }
            return vec4(rgb, a)
        }
    }

    // Custom shape instances: a regular polygon (sides, radius, angle)
    // around (x, y) in shape space (0..1, y up), coloured from the centre
    // colour to the edge colour, optionally textured from the feedback
    // buffer and outlined. The quad covers the polygon's circle.
    mod.draw.DrawMilkShape = mod.std.set_type_default() do #(DrawMilkShape::script_shader(vm)){
        ..mod.draw.DrawMilkBase

        pixel: fn() {
            // Pixel position relative to the centre, in radius units.
            let px = self.pos * self.rect_size + self.rect_pos
            let target = self.u_pixels.xy
            let center = vec2(self.sh0.x, 1.0 - self.sh0.y) * target
            let radius_px = max(self.sh0.z * min(target.x, target.y) * 0.5, 0.0001)
            let rel = (px - center) / radius_px
            let r = length(rel)
            let sides = max(floor(self.sh1.x + 0.5), 3.0)
            let seg = 6.283185307 / sides
            // Angle of the pixel from the first vertex (y up in shape space).
            let a0 = self.sh0.w + 0.785398163
            let a = atan2(-rel.y, rel.x) - a0
            let am = a - floor(a / seg) * seg
            // Distance from centre to the polygon edge along this ray.
            let edge_r = cos(seg * 0.5) / max(cos(am - seg * 0.5), 0.0001)
            let t = r / edge_r
            let aa = 1.5 / radius_px
            let inside = 1.0 - smoothstep(1.0 - aa, 1.0, t)
            var c = mix(self.c_in, self.c_out, clamp(t, 0.0, 1.0))
            if self.sh1.y > 0.5 {
                // Textured: the feedback buffer around the screen centre.
                let zoom = max(abs(self.sh1.z), 0.0001)
                let ca = cos(self.sh1.w)
                let sa = sin(self.sh1.w)
                let q = vec2(rel.x * ca - rel.y * sa, rel.x * sa + rel.y * ca) * self.sh0.z / zoom
                let uv = vec2(0.5 + q.x * 0.5 * self.u_aspect.y, 0.5 + q.y * 0.5)
                let tex = self.tex_main.sample_repeat(uv).xyz
                c = vec4(c.xyz * tex, c.w)
            }
            var alpha = clamp(c.w, 0.0, 1.0) * inside
            var rgb = clamp(c.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)) * alpha
            // Outline: 1 px, or 4 px when thick.
            if self.c_border.w > 0.001 {
                let width = (1.0 + self.sh2.y * 3.0) / radius_px / max(edge_r, 0.0001)
                let band = smoothstep(1.0 - width - aa, 1.0 - width, t) * (1.0 - smoothstep(1.0, 1.0 + aa, t))
                let ba = clamp(self.c_border.w, 0.0, 1.0) * band
                rgb = rgb * (1.0 - ba) + clamp(self.c_border.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)) * ba
                alpha = alpha * (1.0 - ba) + ba
            }
            if self.sh2.x > 0.5 {
                return vec4(rgb, 0.0)
            }
            return vec4(rgb, alpha)
        }
    }

    // A solid rectangle (the outer and inner borders).
    mod.draw.DrawMilkFill = mod.std.set_type_default() do #(DrawMilkFill::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let a = clamp(self.color.w, 0.0, 1.0)
            return vec4(clamp(self.color.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)) * a, a)
        }
    }

    // The final picture into the view, and the cross-fade between two
    // presets' pictures during a transition.
    mod.draw.DrawMilkBlit = mod.std.set_type_default() do #(DrawMilkBlit::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_a: texture_2d(float)
        tex_b: texture_2d(float)
        // (mix toward b, _, _, _)
        u_mix: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // A monochrome display: rgb, amount (0 = the picture's own colours).
        u_tint: uniform(vec4(1.0, 1.0, 1.0, 0.0))
        // A colour map: luminance through four stops, blended in by
        // u_map_amount.x (0 = the picture's own colours).
        u_map0: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        u_map1: uniform(vec4(0.33, 0.33, 0.33, 1.0))
        u_map2: uniform(vec4(0.66, 0.66, 0.66, 1.0))
        u_map3: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_map_amount: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // A screen filter over the picture, in this one pass: kind (0 none,
        // 1 CRT, 2 LCD, 3 halftone, 4 etched glass), strength (0 = off),
        // cell size in points (scanline pitch, LCD pixel, halftone dot),
        // CRT curvature. Static: nothing here moves at rest.
        u_filter: uniform(vec4(0.0, 0.0, 3.0, 0.08))
        // The part of the pictures shown: offset xy, size zw (0..1). A
        // picture rendered at another shape (a larger window's) is cropped
        // to fill this quad.
        u_uv: uniform(vec4(0.0, 0.0, 1.0, 1.0))

        pick: fn(uv: vec2) -> vec3 {
            let st = self.u_uv.xy + uv * self.u_uv.zw
            var col = self.tex_a.sample(st).xyz
            if self.u_mix.x > 0.0 {
                col = mix(col, self.tex_b.sample(st).xyz, self.u_mix.x)
            }
            if self.u_map_amount.x > 0.0 {
                let l = clamp(dot(col, vec3(0.30, 0.55, 0.15)), 0.0, 1.0) * 3.0
                var m = mix(self.u_map0.xyz, self.u_map1.xyz, clamp(l, 0.0, 1.0))
                m = mix(m, self.u_map2.xyz, clamp(l - 1.0, 0.0, 1.0))
                m = mix(m, self.u_map3.xyz, clamp(l - 2.0, 0.0, 1.0))
                col = mix(col, m, self.u_map_amount.x)
            }
            if self.u_tint.w > 0.0 {
                let l = dot(col, vec3(0.30, 0.55, 0.15))
                col = mix(col, self.u_tint.xyz * l * 1.25, self.u_tint.w)
            }
            return col
        }

        pixel: fn() {
            let kind = self.u_filter.x
            let amt = self.u_filter.y
            if amt <= 0.0 || kind < 0.5 {
                return vec4(self.pick(self.pos), 1.0)
            }
            let dpi = max(self.draw_pass.dpi_factor, 0.5)
            let cell = max(self.u_filter.z, 0.5) * dpi
            let sz = max(self.rect_size * dpi, vec2(1.0, 1.0))
            let dp = self.pos * sz
            let q = self.pos * 2.0 - vec2(1.0, 1.0)
            if kind < 1.5 {
                // CRT: a bulging tube, scanlines, an RGB mask, bloom, a
                // slight vignette.
                let k = self.u_filter.w * amt
                let uv = (q * (1.0 + k * dot(q, q))) * 0.5 + vec2(0.5, 0.5)
                if uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0 {
                    return vec4(0.0, 0.0, 0.0, 1.0)
                }
                var col = self.pick(uv)
                let scan = 0.5 + 0.5 * cos(dp.y * 6.28318 / cell)
                col = col * (1.0 - 0.38 * amt * (1.0 - scan))
                let m = floor(dp.x - 3.0 * floor(dp.x / 3.0))
                var mask = vec3(0.72, 0.72, 0.72)
                if m < 0.5 {
                    mask = vec3(1.0, 0.72, 0.72)
                } else {
                    if m < 1.5 {
                        mask = vec3(0.72, 1.0, 0.72)
                    } else {
                        mask = vec3(0.72, 0.72, 1.0)
                    }
                }
                col = col * mix(vec3(1.0, 1.0, 1.0), mask, 0.45 * amt)
                col = col + col * col * 0.35 * amt
                col = col * (1.0 - 0.22 * amt * dot(q, q))
                return vec4(col, 1.0)
            }
            if kind < 2.5 {
                // LCD: coarse pixels on a grid, a few levels per channel.
                let cellp = (floor(dp / cell) + vec2(0.5, 0.5)) * cell / sz
                var col = self.pick(cellp)
                let levels = 5.0
                let qc = floor(col * levels + vec3(0.5, 0.5, 0.5)) / levels
                col = mix(col, qc, amt)
                let f = fract(dp / cell)
                let gap = max(1.0 / cell, 0.08)
                let on = step(gap, f.x) * step(gap, f.y)
                col = col * mix(1.0, 0.55, (1.0 - on) * amt)
                return vec4(col, 1.0)
            }
            if kind < 3.5 {
                // Halftone: ink dots sized by the picture's brightness.
                let cellp = (floor(dp / cell) + vec2(0.5, 0.5)) * cell / sz
                let col = self.pick(cellp)
                let l = clamp(dot(col, vec3(0.30, 0.55, 0.15)), 0.0, 1.0)
                let f = fract(dp / cell) - vec2(0.5, 0.5)
                let r = sqrt(l) * 0.72
                let dotm = 1.0 - smoothstep(r - 0.08, r + 0.08, length(f))
                let ink = col / max(max(col.x, max(col.y, col.z)), 0.001)
                let ht = mix(col * 0.12, ink * (0.35 + 0.65 * l), dotm)
                return vec4(mix(col, ht, amt), 1.0)
            }
            // Etched glass: fine diagonal rules and a soft sheen.
            var col = self.pick(self.pos)
            let e = 0.5 + 0.5 * sin((dp.x + dp.y) * 3.14159 / cell)
            col = col * (1.0 - 0.14 * amt * e)
            col = col + vec3(1.0, 0.95, 0.85) * 0.06 * amt * smoothstep(0.8, 0.0, self.pos.x + self.pos.y)
            return vec4(col, 1.0)
        }
    }
}

/// Evaluate a preset's subclass of the base shader `base` (`DrawMilkWarp`
/// ...) with `members`, into a body of its own the VM reclaims once the
/// draw is built (the shader cache keys a compile by its body and text).
/// `kind` names it in diagnostics. The returned value is what
/// `script_apply` takes.
pub fn eval_preset_shader(
    vm: &mut ScriptVm,
    kind: &str,
    base: &str,
    members: &str,
) -> Result<ScriptValue, String> {
    let script_mod = ScriptMod::new("", "", &format!("milkdrop://{kind}"), 0, 0, &format!("use mod.std.*\nuse mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw\nmod.draw.{base}{{\n{members}}}\n"), &[]);
    vm.capture_errors();
    let value = vm.eval_transient(script_mod);
    let errors = vm.take_errors();
    if value.is_err() || !errors.is_empty() {
        return Err(if errors.is_empty() { "evaluation failed".into() } else { errors.join("; ") });
    }
    Ok(value)
}

/// Compile the evaluated shader `value` for the Metal backend's source
/// stage without drawing: every type or name error the shader has, "" for
/// none. Used by tests and the corpus checker.
pub fn shader_compile_errors(vm: &mut ScriptVm, value: ScriptValue) -> String {
    let script_mod = ScriptMod::new("", "", "milkdrop://check", 0, 0, "fn(s) { return mod.shader.test_compile_draw_errors(s) }", &[]);
    vm.capture_errors();
    let check = vm.eval_transient(script_mod);
    let result = vm.call(check, &[value]);
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return errors.join("; ");
    }
    vm.heap().string_with(result, |_h, s| s.to_string()).unwrap_or_else(|| "compile returned no string".into())
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkBase {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkMesh {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkWarp {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkComp {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkBlur {
    #[deref]
    pub draw_super: DrawQuad,
}

/// One wave line segment. In CPU mode `a`/`b` hold the end positions in
/// .xy and `ca`/`cb` their colours; in GPU mode `a`/`b` hold (sample,
/// value1, value2) for the per-point code.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkWave {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub a: Vec4f,
    #[live]
    pub b: Vec4f,
    #[live]
    pub ca: Vec4f,
    #[live]
    pub cb: Vec4f,
}

/// One custom shape instance.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkShape {
    #[deref]
    pub draw_super: DrawQuad,
    /// (x, y, rad, ang)
    #[live]
    pub sh0: Vec4f,
    /// (sides, textured, tex_zoom, tex_ang)
    #[live]
    pub sh1: Vec4f,
    /// (additive, thick outline, _, _)
    #[live]
    pub sh2: Vec4f,
    #[live]
    pub c_in: Vec4f,
    #[live]
    pub c_out: Vec4f,
    #[live]
    pub c_border: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkFill {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMilkBlit {
    #[deref]
    pub draw_super: DrawQuad,
}

#[cfg(test)]
pub(crate) mod tests {
    use makepad_widgets::makepad_draw;
    use makepad_widgets::makepad_platform::makepad_script::script_eval;
    use makepad_widgets::*;

    /// A script VM with the draw shaders and these shaders registered.
    pub(crate) fn with_shader_vm(f: impl FnOnce(&mut ScriptVm)) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            vm.capture_errors();
            makepad_draw::script_mod(vm);
            super::script_mod(vm);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "registration errors: {errors:#?}");
            f(vm);
        });
    }

    /// Compile errors of `base` subclassed with `members` ("" = none),
    /// evaluated like the renderer does.
    pub(crate) fn check(vm: &mut ScriptVm, base: &str, members: &str) -> String {
        match super::eval_preset_shader(vm, "test", base, members) {
            Ok(value) => super::shader_compile_errors(vm, value),
            Err(e) => e,
        }
    }

    #[test]
    fn the_base_shaders_compile() {
        with_shader_vm(|vm| {
            for name in ["DrawMilkMesh", "DrawMilkWarp", "DrawMilkComp", "DrawMilkBlur", "DrawMilkBlit", "DrawMilkWave", "DrawMilkShape", "DrawMilkFill"] {
                let e = check(vm, name, "");
                assert!(e.is_empty(), "{name}: {e}");
            }
            let bad = check(vm, "DrawMilkWarp", "milk_body: fn(uv_in: vec2, uv_orig: vec2, rad: float, ang: float) -> vec3 { return nothing_here }");
            assert!(!bad.is_empty(), "a broken body must report an error");
            let _ = script_eval!(vm, { 0 });
        });
    }

    #[test]
    fn translated_shaders_compile() {
        use crate::hlsl::{translate, ShaderKind};
        let cases = [
            ("shader_body { ret = tex2D(sampler_main, uv).xyz; ret *= 0.97; }", ShaderKind::Warp),
            ("float2 rot(float2 p, float a) { return float2(p.x*cos(a) - p.y*sin(a), p.x*sin(a) + p.y*cos(a)); } static float3 k = float3(q1, q2, 1); shader_body { float3 c = tex2D(sampler_fc_main, rot(uv - 0.5, time) + 0.5); c += GetBlur1(uv) * k; ret = c.y > 0.5 ? c : 1 - c; ret.x += 0.1; for (int i = 0; i < 3; i++) { ret *= 0.99; } }", ShaderKind::Warp),
            ("sampler sampler_worms; shader_body { float2x2 m = float2x2(1,0,0,1); ret = tex2D(sampler_worms, mul(uv, m)).xyz * hue_shader + tex3D(sampler_noisevol_hq, float3(uv, time)).xyz + lum(GetBlur3(uv)); float a[3]; a[1] = 2; ret *= a[1]; }", ShaderKind::Comp),
            ("float3x3 rx(float a) { return float3x3(1, 0, 0, 0, cos(a), -sin(a), 0, sin(a), cos(a)); } shader_body { float3x3 r = rx(time); float3 p = mul(float3(uv - 0.5, 1), r); p = mul(mul(r, r), p) - r[2] * 0.1; ret = tex2D(sampler_main, p.xy / p.z + 0.5).xyz; }", ShaderKind::Warp),
        ];
        with_shader_vm(|vm| {
            for (src, kind) in cases {
                let t = translate(src, kind).unwrap();
                let base = if kind == ShaderKind::Warp { "DrawMilkWarp" } else { "DrawMilkComp" };
                let e = check(vm, base, &t.members);
                assert!(e.is_empty(), "{src}\n{}\n{e}", t.members);
            }
        });
    }

    #[test]
    fn the_betelgeuse_warp_compiles_alone() {
        let Some(path) = std::env::var_os("MILKDROP_ONE") else { return };
        let preset = crate::Preset::parse(&std::fs::read(path).unwrap());
        let t = crate::hlsl::translate(preset.warp_shader.as_deref().unwrap(), crate::hlsl::ShaderKind::Warp).unwrap();
        with_shader_vm(|vm| {
            let e = check(vm, "DrawMilkWarp", &t.members);
            println!("RESULT [{e}]");
        });
    }

    #[test]
    fn gpu_equations_compile() {
        use crate::eel_gpu::{per_point, per_vertex, GpuPlan};
        with_shader_vm(|vm| {
            let GpuPlan::Gpu(v) = per_vertex("zoom = zoom + 0.05*sin(rad*8 + time) + if(above(x, 0.5), 0.01, 0); rot = rot + q1*ang; t = rand(4); dx = dx + t*0.001; loop(3, cx = cx + 0.001); a = (x > 0.5) && (y < 0.2); sx = sx + a*0.01 + x^2 + y%3; sy = sy + ((q2 * 7) & 3) * 0.01 + ((q3 | 4) == 5) * 0.02; cy |= 0;") else { panic!() };
            let e = check(vm, "DrawMilkMesh", &v);
            assert!(e.is_empty(), "{v}\n{e}");
            let GpuPlan::Gpu(p) = per_point("x = 0.5 + value1*sin(sample*6.28)*t1; y = 0.5 + value2; r = sample; a = above(sample, 0.5) ? 1 : 0.2;") else { panic!() };
            let e = check(vm, "DrawMilkWave", &p);
            assert!(e.is_empty(), "{p}\n{e}");
        });
    }

    /// Whole-corpus translate + compile, when `MILKDROP_CORPUS` names a
    /// preset directory. Prints counts and the commonest failure causes.
    #[test]
    fn corpus_shaders_translate_and_compile() {
        use crate::hlsl::{translate, ShaderKind};
        use std::collections::HashMap;
        let Some(dir) = std::env::var_os("MILKDROP_CORPUS") else { return };
        let mut files = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p)
                } else if p.extension().is_some_and(|x| x == "milk") {
                    files.push(p)
                }
            }
        }
        files.sort();
        let limit: usize = std::env::var("MILKDROP_CORPUS_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
        let mut stats: HashMap<&str, [usize; 3]> = HashMap::new(); // [total, translated, compiled]
        let mut causes: HashMap<String, usize> = HashMap::new();
        let mut compile_causes: HashMap<String, usize> = HashMap::new();
        // [empty, cpu, gpu, gpu compiled]
        let mut eq_stats: HashMap<&str, [usize; 4]> = HashMap::new();
        let mut eq_causes: HashMap<String, usize> = HashMap::new();
        let start = std::time::Instant::now();
        with_shader_vm(|vm| {
            for path in files.iter().take(limit) {
                let preset = crate::Preset::parse(&std::fs::read(path).unwrap());
                // Equations: GPU plan and compile.
                {
                    use crate::eel_gpu::{per_point, per_vertex, GpuPlan};
                    let mut plans = vec![("per_vertex", per_vertex(&preset.per_pixel), "DrawMilkMesh")];
                    for w in preset.waves.iter().filter(|w| w.enabled()) {
                        plans.push(("per_point", per_point(&w.per_point), "DrawMilkWave"));
                    }
                    for (label, plan, base) in plans {
                        let entry = eq_stats.entry(label).or_default();
                        match plan {
                            GpuPlan::Empty => entry[0] += 1,
                            GpuPlan::Cpu(reason) => {
                                entry[1] += 1;
                                let key = format!("{label}: {}", reason.split(':').next().unwrap_or(""));
                                *eq_causes.entry(key).or_default() += 1;
                                if let Some(vars) = reason.split(": ").nth(1) {
                                    for v in vars.split(", ") {
                                        *eq_causes.entry(format!("  {label} carries {v}")).or_default() += 1;
                                    }
                                }
                            }
                            GpuPlan::Gpu(members) => {
                                entry[2] += 1;
                                let e = check(vm, base, &members);
                                if e.is_empty() {
                                    entry[3] += 1;
                                } else {
                                    let key: String = format!("{label}: {e}").chars().take(90).collect();
                                    *compile_causes.entry(key).or_default() += 1;
                                }
                            }
                        }
                    }
                }
                for (label, src, kind, base) in [
                    ("warp", &preset.warp_shader, ShaderKind::Warp, "DrawMilkWarp"),
                    ("comp", &preset.comp_shader, ShaderKind::Comp, "DrawMilkComp"),
                ] {
                    let Some(src) = src else { continue };
                    let entry = stats.entry(label).or_default();
                    entry[0] += 1;
                    match translate(src, kind) {
                        Ok(t) => {
                            entry[1] += 1;
                            let src_text = t.members.clone();
                            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(vm, base, &src_text)));
                            if std::env::var_os("MILKDROP_CANARY").is_some() {
                                let canary = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(vm, "DrawMilkWarp", "milk_body: fn(uv_in: vec2, uv_orig_in: vec2, rad_in: float, ang_in: float) -> vec3 { return vec3(1.0, 0.0, 0.0) }")));
                                if !matches!(&canary, Ok(e) if e.is_empty()) {
                                    println!("CANARY FAILED after {} bodies at {}", vm.body_count(), path.display());
                                    panic!("canary");
                                }
                            }
                            let e = match caught {
                                Ok(e) => e,
                                Err(_) => {
                                    println!("PANIC after {} bodies: {}\n{}", vm.body_count(), path.display(), src_text);
                                    "compiler panic".to_string()
                                }
                            };
                            if e.is_empty() {
                                entry[2] += 1;
                            } else {
                                let key: String = e.chars().take(90).collect();
                                *compile_causes.entry(key).or_default() += 1;
                                if std::env::var_os("MILKDROP_CORPUS_DUMP").is_some() {
                                    println!("== {}\n{}\n{}\n", path.display(), t.members, e);
                                }
                            }
                        }
                        Err(e) => {
                            let key: String = e.message.chars().take(70).collect();
                            *causes.entry(key).or_default() += 1;
                        }
                    }
                }
            }
        });
        println!("corpus shaders ({:?}):", start.elapsed());
        for (label, [t, tr, c]) in &stats {
            println!("  {label}: {t} shaders, {tr} translated, {c} compiled");
        }
        for (label, [empty, cpu, gpu, ok]) in &eq_stats {
            println!("  {label}: {empty} empty, {cpu} CPU fallback, {gpu} GPU ({ok} compiled)");
        }
        let mut eq_causes: Vec<_> = eq_causes.into_iter().collect();
        eq_causes.sort_by(|a, b| b.1.cmp(&a.1));
        println!("CPU fallback reasons:");
        for (c, n) in eq_causes.iter().take(40) {
            println!("  {n:5} {c}");
        }
        let mut causes: Vec<_> = causes.into_iter().collect();
        causes.sort_by(|a, b| b.1.cmp(&a.1));
        println!("translate failures:");
        for (c, n) in causes.iter().take(25) {
            println!("  {n:5} {c}");
        }
        let mut compile_causes: Vec<_> = compile_causes.into_iter().collect();
        compile_causes.sort_by(|a, b| b.1.cmp(&a.1));
        println!("compile failures:");
        for (c, n) in compile_causes.iter().take(25) {
            println!("  {n:5} {c}");
        }
    }
}
