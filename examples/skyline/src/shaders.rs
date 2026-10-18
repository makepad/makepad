//! The scene's own draw shaders: the city glow, the mountain ranges (and the front treeline) and the
//! ground band, plus the sky backdrop the app shows behind a view.
//!
//! Every scene shader writes the scene pass's two RGBA16F targets, both blending premultiplied-over:
//! `fb0` the premultiplied colour and coverage, `fb1` the light and the same coverage. So a nearer
//! silhouette hides the lights behind it, and the final pass adds the light back on top. Shaders
//! of other stages (the city blit, the parts) join the scene with `..mod.draw.SkyScene`.
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
        cols: texture_2d(float)
        tree_cov: texture_2d(float)
        haze: uniform(0.5)
        glow: uniform(0.5)
        sil: uniform(vec3(0.004, 0.006, 0.012))
        haze_col: uniform(vec3(0.16, 0.18, 0.28))

        // the fragment's centre in pass pixels
        frag: fn() -> vec2 {
            return self.rect_pos + self.pos * self.rect_size
        }
        // radians per pixel of the raw view
        px0: fn() -> float {
            return self.view_span.y * 6.2831853 / self.view_size.x
        }
        // raw elevation (rad) of pass row y (mirrored in the reflection run)
        ang_raw: fn(y: float) -> float {
            let a = (self.view_horizon * self.view_size.y - y) * self.px0()
            return mix(a, 0.0 - a, self.mirror)
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
        cover: fn(d: float, px: float) -> float {
            return clamp(d / px + 0.5, 0.0, 1.0)
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
            let pp = self.frag()
            let g = self.col_row(pp.x, 6.0)
            let ang = self.ang_raw(pp.y) * g.z
            let px = self.px0() * g.z
            let k = self.k
            let r = self.col_row(pp.x, min(k, 5.0))
            let sil = self.sil
            var tm = 0.0
            if k > 1.5 {
                let tr = self.tree_cov.sample_nearest(pp / self.view_size)
                let ch = vec4(step(abs(k - 2.0), 0.5), step(abs(k - 3.0), 0.5), step(abs(k - 4.0), 0.5), step(4.5, k))
                tm = r.w * max(self.cover(r.z - ang, 1.5 * px), dot(tr, ch))
                if ang < r.y - px {
                    tm = r.w
                }
            }
            if k > 4.5 {
                return vec4(sil * (0.6 * tm), tm)
            }
            let t = (4.0 - k) / 4.0
            let hz = self.haze_col
            let hz2 = self.haze * self.haze * 0.35
            let m = self.cover(r.x - ang, px)
            let ftop = clamp(mix(0.012 + 0.01 * t, 0.30 * t * t + 0.06 * t + 0.02, hz2), 0.0, 1.0)
            let fog = pow(1.0 - smoothstep(0.0, 0.55 * self.hk + 0.004, ang), 1.4)
            let fogc = mix(hz * (0.7 + 0.25 * t), hz * (1.6 + 1.6 * t) + vec3(0.06, 0.07, 0.09) * t, hz2)
            let rock = mix(mix(sil, fogc, ftop), fogc * (1.1 + 0.2 * t), fog * mix(0.1, 0.9, hz2) * (0.25 + 0.75 * t))
            return vec4(sil * (0.6 * tm) + rock * (m * (1.0 - tm)), tm + m * (1.0 - tm))
        }
        fragment: fn(){
            let c = self.pixel()
            self.fb0 = c
            self.fb1 = vec4(0.0, 0.0, 0.0, c.w)
        }
    }

    // The ground band: the near ground rise over land, flat sea below the horizon.
    mod.draw.DrawSkyGround = mod.std.set_type_default() do #(DrawSkyGround::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        pixel: fn(){
            let pp = self.frag()
            let g = self.col_row(pp.x, 6.0)
            let gm = self.cover(g.y - self.ang_raw(pp.y) * g.z, self.px0() * g.z)
            return vec4(mix(self.sil * 0.5, self.sil * 1.6, g.x) * gm, gm)
        }
        fragment: fn(){
            let c = self.pixel()
            self.fb0 = c
            self.fb1 = vec4(0.0, 0.0, 0.0, c.w)
        }
    }

    // The night sky behind a view (the page's CSS gradient), with a hard step to the sea at the horizon.
    mod.draw.DrawSkyBackdrop = mod.std.set_type_default() do #(DrawSkyBackdrop::script_shader(vm)){
        ..mod.draw.DrawQuad,
        horizon: uniform(0.8)
        pixel: fn(){
            let y = self.pos.y
            let h = self.horizon
            if y >= h {
                return #x0a0d16
            }
            let f = y / h
            if f < 0.45 {
                return mix(#x050813, #x0a1230, f / 0.45)
            }
            if f < 0.8 {
                return mix(#x0a1230, #x1b2547, (f - 0.45) / 0.35)
            }
            if f < 0.95 {
                return mix(#x1b2547, #x4a4258, (f - 0.8) / 0.15)
            }
            return mix(#x4a4258, #x8a5a55, (f - 0.95) / 0.05)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyGlow {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyRange {
    #[deref]
    pub draw_super: DrawQuad,
    /// The range (0 farthest .. 4 nearest; 5 the front treeline).
    #[live]
    pub k: f32,
    /// Its peak height (rad).
    #[live]
    pub hk: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyGround {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyBackdrop {
    #[deref]
    pub draw_super: DrawQuad,
}
