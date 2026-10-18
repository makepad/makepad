//! The scene's own draw shaders: the city glow, the mountain ranges (and the front treeline) and the
//! ground band, plus the sky backdrop the app shows behind a view.
//!
//! Every scene shader writes the scene pass's two RGBA16F targets, both blending premultiplied-over:
//! `fb0` the premultiplied colour and coverage, `fb1` the light and the same coverage. So a nearer
//! silhouette hides the lights behind it, and the final pass adds the light back on top. Shaders
//! of other stages (the city, the parts, the final pass) join the scene with `..mod.draw.SkyScene`,
//! which also gives them the off-centre view, the city camera and the page's integer hashes
//! (checked against `rng.rs` on the GPU by `tests/checks.rs`).
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // What every scene shader shares: the second output, the view, the column data and helpers.
    // The view uniforms are set by `SceneCtx::set_uniforms`; a parameter's uniform has its key's name.
    mod.draw.SkyScene = {
        fb1: fragment_output(1, vec4f)
        // the scene pass's size in pixels
        view_size: uniform(vec2(1.0, 1.0))
        // the first azimuth and the span the view shows (0..1 = 0..360 degrees)
        view_span: uniform(vec2(0.0, 1.0))
        // the horizon line, a fraction of the height from the top
        view_horizon: uniform(0.8)
        // column texture width and height, margin columns, column count
        col_geom: uniform(vec4(1.0, 7.0, 0.0, 1.0))
        // 1 in the reflection run: the scene seen mirrored about the horizon
        mirror: uniform(0.0)
        // the upright run's horizon row less this run's (px): 0, and under a pixel in the reflection run (`ViewSpec::mirrored`)
        up_dy: uniform(0.0)
        cols: texture_2d(float)
        tree_cov: texture_2d(float)
        haze: uniform(0.5)
        glow: uniform(0.5)
        sil: uniform(vec3(0.004, 0.006, 0.012))
        haze_col: uniform(vec3(0.16, 0.18, 0.28))
        seed: uniform(7.0)
        ct_c: uniform(0.55)
        ct_r: uniform(1.0)
        off: uniform(0.0)
        off_dir: uniform(0.55)

        // the fragment's centre in pass pixels
        frag: fn() -> vec2 { return self.rect_pos + self.pos * self.rect_size }
        // radians per pixel of the raw view
        px0: fn() -> float { return self.view_span.y * 6.2831853 / self.view_size.x }
        // raw elevation (rad) of pass row y (mirrored in the reflection run)
        ang_raw: fn(y: float) -> float {
            let a = (self.view_horizon * self.view_size.y - y) * self.px0()
            return mix(a, 0.0 - a, self.mirror)
        }
        // the fragment's place in the upright run's targets (the trees, the city): there, the row of its own raw elevation
        up_uv: fn() -> vec2 {
            let pp = self.frag()
            return vec2(pp.x, self.view_horizon * self.view_size.y + self.up_dy - self.ang_raw(pp.y) / self.px0()) / self.view_size
        }
        // row r of the column under pass x: rows 0-4 the ranges (ridge, tree foot, canopy top,
        // density), 5 the front treeline, 6 (land, ground rise, off-centre factor t, city glow)
        col_row: fn(x: float, r: float) -> vec4 {
            let c = floor(x) + self.col_geom.z
            let q = floor(c / self.col_geom.x)
            let xx = c - q * self.col_geom.x
            return self.cols.sample_nearest(vec2((xx + 0.5) / self.col_geom.x, (r + 7.0 * q + 0.5) / self.col_geom.y))
        }
        // coverage of a signed distance d over a pixel of size px
        cover: fn(d: float, px: float) -> float { return clamp(d / px + 0.5, 0.0, 1.0) }
        // the fragment's raw view: azimuth u (0..1), raw elevation (rad), radians per pixel
        view_raw: fn() -> vec3 {
            let pp = self.frag()
            return vec3(fract(self.view_span.x + pp.x / self.view_size.x * self.view_span.y), self.ang_raw(pp.y), self.px0())
        }
        // the page's `pixView`: raw view vr re-aimed for the off-centre camera (the scenery is a
        // ring of radius 1, the camera `off` from its centre toward `off_dir`): the world azimuth
        // the pixel sees, elevation and pixel size scaled by the distance t to that point
        pix_view: fn(vr: vec3) -> vec3 {
            let d = vec2(cos(vr.x * 6.2831853), sin(vr.x * 6.2831853))
            let c = self.off * vec2(cos(self.off_dir * 6.2831853), sin(self.off_dir * 6.2831853))
            let cd = dot(c, d)
            let t = 0.0 - cd + sqrt(max(cd * cd - dot(c, c) + 1.0, 0.0001))
            let q = c + t * d
            return vec3(fract(atan2(q.y, q.x) / 6.2831853), vr.y * t, vr.z * t)
        }
        // the city camera: downtown's distance (blocks) and direction, the real camera offset (blocks)
        city_d: fn() -> float { return 28.0 * self.ct_r }
        city_dir: fn() -> vec2 { return vec2(cos(self.ct_c * 6.2831853), sin(self.ct_c * 6.2831853)) }
        city_cam: fn() -> vec2 { return (0.8 * self.off * self.city_d()) * vec2(cos(self.off_dir * 6.2831853), sin(self.off_dir * 6.2831853)) }
        // the page's `prCityView`: world point w (m; 70 m a block) from the city camera: raw azimuth, distance (m)
        city_view: fn(w: vec2) -> vec2 {
            let q = w - self.city_cam() * 70.0
            return vec2(fract(atan2(q.y, q.x) / 6.2831853), length(q))
        }
        // the page's integer hashes, bit-exact with `rng.rs` (u32 arithmetic wraps on every backend;
        // a negative i32 keeps its bits as u32, so `+ 4096u` is the page's `int(id) + 4096`)
        hu: fn(x_in: u32) -> u32 {
            var x = x_in
            x = x ^ (x >> 16u)
            x = x * 2146121005u
            x = x ^ (x >> 15u)
            x = x * 2221713035u
            return x ^ (x >> 16u)
        }
        // `rnd`'s and `hcell`'s common form: hu(a K1 ^ (b K2 + c K3 + seed K4))
        hash3: fn(a: u32, b: u32, c: u32) -> u32 {
            return self.hu((a * 2654435761u) ^ ((b * 2246822507u) + (c * 3266489909u) + (u32(i32(self.seed)) * 668265263u)))
        }
        // `rnd(id, st, ly)`: 0..1 in steps of 2^-24
        rnd: fn(id: float, st: float, ly: float) -> float {
            return f32(self.hash3(u32(i32(id)) + 4096u, u32(i32(st)), u32(i32(ly))) >> 8u) / 16777216.0
        }
        // `hcell`: the hash of integer cell c, layer k (the city's lots, the trees' and the town's cells)
        hcell: fn(c: vec2, k: float) -> u32 { return self.hash3(u32(i32(c.x)) + 8192u, u32(i32(c.y)) + 8192u, u32(i32(k))) }
        // a hash's four bytes as 0..1
        hb4: fn(h: u32) -> vec4 { return vec4(f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u), f32(h >> 24u)) * 0.003921569 }
        // the page's `kelvin`: a black-body-like light colour at k K, brightest channel 1 (the city's windows, the props' lights)
        kelvin: fn(k: float) -> vec3 {
            let t = clamp(k, 1000.0, 12000.0) / 100.0
            if t > 66.0 { return vec3(clamp(1.292936 * pow(t - 60.0, -0.1332047592), 0.0, 1.0), clamp(1.129891 * pow(t - 60.0, -0.0755148492), 0.0, 1.0), 1.0) }
            var b = 1.0; if t < 66.0 { b = if t <= 19.0 { 0.0 } else { clamp(0.54320679 * log(t - 10.0) - 1.19625409, 0.0, 1.0) } }
            return vec3(1.0, clamp(0.39008158 * log(t) - 0.63184144, 0.0, 1.0), b)
        }
    }

    // The glow dome over downtown: light only, behind everything (the ranges hide it).
    mod.draw.DrawSkyGlow = mod.std.set_type_default() do #(DrawSkyGlow::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        // its e-folding rate in elevation: the camera's distance to downtown over the tower height
        dome_k: uniform(1.0)
        pixel: fn(){
            let pp = self.frag()
            let g = self.col_row(pp.x, 6.0)
            let a = self.ang_raw(pp.y)
            let dome = exp(0.0 - max(a, 0.0) * self.dome_k) * g.w * self.glow * smoothstep(0.0 - 0.012, 0.0, a)
            return vec4(vec3(1.0, 0.55, 0.25) * (dome * 0.10), 0.0)
        }
        fragment: fn(){
            self.fb0 = vec4(0.0, 0.0, 0.0, 0.0)
            self.fb1 = self.pixel()
        }
    }

    // A mountain range k (0 farthest .. 4 nearest) over its height band, with its forest: rock in the
    // night's tonal order (farther and lower is hazier), then the trees over it (the darkest thing
    // on a flank): solid below the forest's foot line, the canopy, the single trees. k = 5 is the
    // front treeline, trees only.
    mod.draw.DrawSkyRange = mod.std.set_type_default() do #(DrawSkyRange::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        k: 0.0
        hk: 0.2
        pixel: fn(){
            let pp = self.frag(); let g = self.col_row(pp.x, 6.0); let k = self.k; let sil = self.sil
            let ang = self.ang_raw(pp.y) * g.z; let px = self.px0() * g.z; let r = self.col_row(pp.x, min(k, 5.0))
            var tm = 0.0
            if k > 1.5 {
                // the trees are drawn upright once a picture: the reflection run reads them at the mirrored row
                let tr = self.tree_cov.sample_nearest(self.up_uv())
                let ch = vec4(step(abs(k - 2.0), 0.5), step(abs(k - 3.0), 0.5), step(abs(k - 4.0), 0.5), step(4.5, k))
                tm = r.w * max(self.cover(r.z - ang, 1.5 * px), dot(tr, ch))
                if ang < r.y - px { tm = r.w }
            }
            if k > 4.5 { return vec4(sil * (0.6 * tm), tm) }
            let t = (4.0 - k) / 4.0; let hz = self.haze_col; let hz2 = self.haze * self.haze * 0.35; let m = self.cover(r.x - ang, px)
            let ftop = clamp(mix(0.012 + 0.01 * t, 0.30 * t * t + 0.06 * t + 0.02, hz2), 0.0, 1.0)
            let fog = pow(1.0 - smoothstep(0.0, 0.55 * self.hk + 0.004, ang), 1.4)
            let fogc = mix(hz * (0.7 + 0.25 * t), hz * (1.6 + 1.6 * t) + vec3(0.06, 0.07, 0.09) * t, hz2)
            let rock = mix(mix(sil, fogc, ftop), fogc * (1.1 + 0.2 * t), fog * mix(0.1, 0.9, hz2) * (0.25 + 0.75 * t))
            return vec4(sil * (0.6 * tm) + rock * (m * (1.0 - tm)), tm + m * (1.0 - tm))
        }
        fragment: fn(){ let c = self.pixel(); self.fb0 = c; self.fb1 = vec4(0.0, 0.0, 0.0, c.w) }
    }

    // The ground band: the near ground rise over land, flat sea below the horizon.
    mod.draw.DrawSkyGround = mod.std.set_type_default() do #(DrawSkyGround::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        pixel: fn(){
            let pp = self.frag(); let g = self.col_row(pp.x, 6.0)
            let gm = self.cover(g.y - self.ang_raw(pp.y) * g.z, self.px0() * g.z)
            return vec4(mix(self.sil * 0.5, self.sil * 1.6, g.x) * gm, gm)
        }
        fragment: fn(){ let c = self.pixel(); self.fb0 = c; self.fb1 = vec4(0.0, 0.0, 0.0, c.w) }
    }

    // The night sky behind a view (the page's CSS gradient), with a hard step to the sea at the horizon.
    mod.draw.DrawSkyBackdrop = mod.std.set_type_default() do #(DrawSkyBackdrop::script_shader(vm)){
        ..mod.draw.DrawQuad,
        horizon: uniform(0.8)
        pixel: fn(){
            let y = self.pos.y; let h = self.horizon; let f = y / h
            if y >= h { return #x0a0d16 }
            if f < 0.45 { return mix(#x050813, #x0a1230, f / 0.45) }
            if f < 0.8 { return mix(#x0a1230, #x1b2547, (f - 0.45) / 0.35) }
            if f < 0.95 { return mix(#x1b2547, #x4a4258, (f - 0.8) / 0.15) }
            return mix(#x4a4258, #x8a5a55, (f - 0.95) / 0.05)
        }
    }
}

#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyGlow { #[deref] pub draw_super: DrawQuad }

/// `k`: the range (0 farthest .. 4 nearest; 5 the front treeline); `hk`: its peak height (rad).
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyRange { #[deref] pub draw_super: DrawQuad, #[live] pub k: f32, #[live] pub hk: f32 }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyGround { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyBackdrop { #[deref] pub draw_super: DrawQuad }
