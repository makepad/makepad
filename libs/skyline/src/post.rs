//! After the scene: scene + mirror -> water (the light plus what the sea mirrors) -> blur H -> final (blur V, plus the
//! colour, then the display tone shoulder; an export keeps the raw HDR). A stage that would do nothing is left out;
//! the final samples the light target the chain ends in. The blur takes the reflections with the lights, as the page's.
use crate::light::SkyLight;
use crate::params::{Params, P};
use crate::render::{bind_texture, SceneCtx, SceneTarget, ViewSpec};
use crate::rng::TAU;
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // A gaussian of `sigma` texels of `scene_light` along d about the texel under uv (the page's blur: at most 129 taps,
    // wider steps past that), clamped to the edge texels (the scene has the blur's margin, so they are never reached).
    mod.draw.SkyBlur = {
        scene_light: texture_2d(float)
        sigma: uniform(0.0)
        gauss: fn(uv: vec2, d: vec2) -> vec4 {
            let r = ceil(3.0 * self.sigma); let s = max(1.0, r / 64.0); let n = min(r, 64.0); let ts = self.scene_light.size(); let lo = vec2(0.5, 0.5) / ts
            var acc = vec4(0.0, 0.0, 0.0, 0.0); var ws = 0.0
            for i in 0..u32(2.0 * n + 1.0) {
                let o = (float(i) - n) * s; let w = exp(0.0 - o * o / (2.0 * self.sigma * self.sigma)); let q = (floor(uv * ts) + 0.5 + d * o) / ts
                acc = acc + self.scene_light.sample(clamp(q, lo, vec2(1.0, 1.0) - lo)) * w; ws = ws + w
            }
            return acc / ws
        }
    }

    // The final composite: the scene's colour plus its light (premultiplied, so light also shows over the sky; blurred
    // vertically when `sigma` > 0), then the tone shoulder above 1.0 unless `tone` is 0.
    mod.draw.DrawSkyFinal = mod.std.set_type_default() do #(DrawSkyFinal::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyBlur,
        scene_color: texture_2d(float)
        tone: uniform(1.0)
        // the view inside the scene's targets (their margin cropped off): offset, size (uv)
        crop: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        pixel: fn(){
            let uv = self.crop.xy + self.pos * self.crop.zw; let c = self.scene_color.sample_nearest(uv); var e = self.scene_light.sample_nearest(uv)
            if self.sigma > 0.0 { e = self.gauss(uv, vec2(0.0, 1.0)) }
            var o = c.xyz + e.xyz; let one = vec3(1.0, 1.0, 1.0)
            if self.tone > 0.5 { o = min(o, one) + (one - exp(min(one - o, vec3(0.0, 0.0, 0.0)))) * 0.4 }
            return vec4(o, c.w)
        }
    }

    // The horizontal step of the blur.
    mod.draw.DrawSkyBlur = mod.std.set_type_default() do #(DrawSkyBlur::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyBlur,
        pixel: fn(){ return self.gauss(self.pos, vec2(1.0, 0.0)) }
    }

    // The scene's light plus what the visible water mirrors, the page's propReflSample and propReflLight: a pixel al
    // below the horizon mirrors elevation al, shifted by its wave row (logarithmic in depth) and its cell around the
    // circle; calm water stretches lights into smears, ripple breaks them into streaks. Strength x Fresnel x depth fade.
    // By day it also mirrors the sky by Fresnel, and (DrawSkyWaterDay) the scene's colour.
    mod.draw.DrawSkyWater = mod.std.set_type_default() do #(DrawSkyWater::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        scene_light: texture_2d(float)
        mirror_light: texture_2d(float)
        // the sky the day's sea mirrors (`SkyLight::sea_texels`: row 0 at +1 degree, row 1 at +15), the rings' first two
        // rows unless the host hands the sky as its weather shows it (`SkyLight::sea_sky`)
        sea_tex: texture_2d(float)
        // the mirror target's horizon (`ViewSpec::mirrored`)
        mirror_horizon: uniform(0.8)
        pr_refl: uniform(0.6)
        pr_ripple: uniform(0.4)
        refl_h: fn(c: float, j: float, ly: float) -> float { return self.rnd(c, j + 8192.0, ly) }
        // the sea's sky row v (1/4 at +1 degree, 3/4 at +15) at azimuth u, as `ring_at` reads the rings
        sea_ring: fn(u: float, v: float) -> vec3 {
            let n = self.sea_tex.size().x; let x = fract(u) * n - 0.5; let i0 = floor(x); let i1 = i0 + 1.0
            let a = self.sea_tex.sample_nearest(vec2((i0 - n * floor(i0 / n) + 0.5) / n, v)).xyz
            let b = self.sea_tex.sample_nearest(vec2((i1 - n * floor(i1 / n) + 0.5) / n, v)).xyz
            return a + (b - a) * (x - i0)
        }
        // the sky the sea mirrors at azimuth u, elevation el (rad), as `sky_at` reads the rings
        sea_sky_at: fn(u: float, el: float) -> vec3 { let a = self.sea_ring(u, 0.25); return a + (self.sea_ring(u, 0.75) - a) * clamp(el / 0.2617994, 0.0, 1.0) }
        // what the water adds of the scene's colour it mirrors at mirror uv q, k its share (the sky's), at azimuth u and
        // depression al: none at night; DrawSkyWaterDay's by day
        mirror_day: fn(q: vec2, k: float, u: float, al: float) -> vec3 { return vec3(0.0, 0.0, 0.0) }
        pixel: fn(){
            let e = self.scene_light.sample_nearest(self.pos); let pp = self.frag(); let vr = self.view_raw(); let v = self.pix_view(vr)
            let g = self.col_row(pp.x, 6.0); let al = 0.0 - v.y; let rip = self.pr_ripple
            if al <= 0.0 { return e }
            // the sea from its near edge out, a lake, standing water on the land (`water_at`)
            let ws = self.water_at(g, v.x, al, v.z); var sea = ws.x
            // over a host's ground the day's sea fades in from the coast (the lights it mirrors at night stay)
            if self.host_ground > 0.5 { sea = sea * (1.0 - self.coast_at(vr.x) * self.day()) }
            // open water (the sea, a lake) mirrors by the Water reflections strength, standing water on the land as a puddle does
            let open = max(sea, ws.y); let water = max(open, ws.z)
            if water < 0.01 { return e }
            let fr = 0.02 + 0.98 * pow(clamp(1.0 - al, 0.0, 1.0), 5.0); let a0 = 0.004; let kz = mix(22.0, 13.0, rip)
            // by day the water mirrors the sky at the mirrored elevation, by its Fresnel term, where nothing mirrored stands in
            // front of it (the mirror's coverage, below), and the scene it mirrors in its colour, over that sky by the same
            // term (`mirror_day`); 0 at night (`day`), where it mirrors only the lights
            let kd = fr * self.day() * water * self.cover(g.y - v.y, v.z); let sky = self.sea_sky_at(vr.x, al) * kd
            if al >= 0.35 { return vec4(e.xyz + sky, e.w) }
            let sm = smoothstep(vec3(0.1, 0.0, 0.5), vec3(0.2, 0.012, 1.4), vec3(al, al, (al + a0) / (kz * v.z))); let w = self.pr_refl * fr * (1.0 - sm.x)
            var wk = w * open
            if ws.z > 0.0 { wk = max(wk, fr * (1.0 - sm.x) * ws.z) }
            if wk < 0.003 { return vec4(e.xyz + sky, e.w) }
            let j = floor(kz * log(al + a0)); let alj = exp((j + 0.5) / kz) - a0; let nu = floor(mix(160.0, 700.0, rip))
            let xc = v.x * nu + self.refl_h(j, 0.0, 2000.0); var ci = floor(xc); let fx = xc - ci; ci = ci - nu * floor(ci / nu)
            var c1 = ci + 1.0; if c1 >= nu { c1 = c1 - nu }
            var c3 = c1; if fx < 0.5 { c3 = ci }
            let sw = clamp(1.0 - 2.4 * rip, 0.05, 1.0)
            let nr = mix(self.refl_h(ci, j, 2001.0), self.refl_h(c1, j, 2001.0), smoothstep(0.5 - 0.5 * sw, 0.5 + 0.5 * sw, fx))
            let q = 1.0 + alj / mix(0.07, 0.15, rip); let dc = 1.0 / (q * q); let amp = (mix(0.004, 0.014, rip) + 0.03 * rip * al) * sm.y * sm.z
            let el = alj / q + dc * (al - alj) + amp * (nr - 0.5) * 2.0
            let du = rip * ((self.refl_h(c3, j, 2002.0) - 0.5) * 2.0 * (0.0012 + 0.03 * al) - 0.8 * (fx - step(0.5, fx)) * 6.2831853 / nu) / 6.2831853
            // the mirror point (raw azimuth vr.x + du, raw elevation -vr.y el / al), which the mirror target shows below its horizon
            let x = ((fract(vr.x + du - self.view_span.x - 0.5 * self.view_span.y + 0.5) - 0.5) / self.view_span.y + 0.5) * self.view_in().x + self.view_pad
            let y = self.mirror_horizon * self.view_size.y - vr.y * el / max(al, 0.00001) / vr.z
            let mq = clamp(vec2(x, y), vec2(0.5, 0.5), self.view_size - vec2(0.5, 0.5)) / self.view_size; let m = self.mirror_light.sample(mq)
            return vec4(e.xyz + m.xyz * (wk * sqrt(dc) * mix(1.0, 0.85, rip) * self.cover(g.y - v.y, v.z)) + sky * (1.0 - m.w) + self.mirror_day(mq, kd, vr.x, al), e.w)
        }
        // one target, but SkyScene declares a second output: written too, so no backend leaves it undefined
        fragment: fn(){ let c = self.pixel(); self.fb0 = c; self.fb1 = c }
    }

    // The water by day (silhouettes by day): DrawSkyWater mirroring the scene's colour as well, over the sky by the sky's
    // own share. The scene's light comes to the water through the host's weather as the sky's does (`SkyLight::sea_sky`):
    // it is scaled by the sea's sky over the rings' at the mirrored elevation, 1 without a weather. Kept apart, so the
    // night's water is the shader it was (any term added to its sum moved two texels of the offcentre baseline on D3D11,
    // at a step of the ripple's arithmetic).
    mod.draw.DrawSkyWaterDay = mod.std.set_type_default() do #(DrawSkyWaterDay::script_shader(vm)){
        ..mod.draw.DrawSkyWater,
        // the mirrored scene's colour (premultiplied, with the light's coverage)
        mirror_color: texture_2d(float)
        mirror_day: fn(q: vec2, k: float, u: float, al: float) -> vec3 {
            let veil = self.sea_sky_at(u, al) / max(self.sky_at(u, al), vec3(0.000001, 0.000001, 0.000001))
            return self.mirror_color.sample(q).xyz * veil * k
        }
    }

    mod.draw.SkyPostBase = #(Post::script_component(vm))
    mod.draw.SkyPost = mod.std.set_type_default() do mod.draw.SkyPostBase{
        draw_final: mod.draw.DrawSkyFinal{} draw_blur: mod.draw.DrawSkyBlur{} draw_water: mod.draw.DrawSkyWater{} draw_water_day: mod.draw.DrawSkyWaterDay{}
    }
}

#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyFinal { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyBlur { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyWater { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyWaterDay { #[deref] pub draw_super: DrawQuad }

/// The scene's targets as post sees them; `mirror`: the mirrored scene, when [`Post::wants_mirror`] asked for it.
pub struct SceneOut<'a> { pub view: &'a ViewSpec, pub scene: &'a SceneTarget, pub mirror: Option<&'a SceneTarget> }

/// `v` with `m` more pixels on every side (the same pixel centres; the horizon moved down by m).
fn widen(v: &ViewSpec, m: usize) -> ViewSpec {
    let (w, h, mf) = (v.w as f64, v.h as f64, m as f64);
    ViewSpec { u0: v.u0 - mf * v.span / w, span: v.span * (w + 2.0 * mf) / w, w: v.w + 2 * m, h: v.h + 2 * m, horizon: (v.horizon * h + mf) / (h + 2.0 * mf), ..*v }
}

/// The blur's margin for view `v`: 3 sigma plus 2 px when the blur shows (the page's), else 0.
fn blur_margin(p: &Params, v: &ViewSpec) -> usize { let s = blur_sigma(p, v); if s > 0.0 { (3.0 * s).ceil() as usize + 2 } else { 0 } }

/// The water's margin for view `v` (px): the farthest its ripple moves a mirror sample sideways, (0.0072 / tau + 0.4 / nu)
/// of a turn at full ripple (`du` in DrawSkyWater, al < 0.2), plus a texel for the filter; 0 for a whole turn, which wraps.
fn water_margin(p: &Params, v: &ViewSpec) -> usize {
    let r = p[P::PrRipple] * (0.0072 / TAU + 0.4 / (160.0 + 540.0 * p[P::PrRipple]).floor());
    if v.span < 1.0 && r > 0.0 { (r * v.w as f64 / v.span).ceil() as usize + 1 } else { 0 }
}

/// The blur's sigma in pixels of view `v` (the page's `blurSigmaPx`: a share of the turn, so every size looks the same),
/// 0 below the 0.3 px the page skips.
pub fn blur_sigma(p: &Params, v: &ViewSpec) -> f64 {
    let s = 0.052 * (1.0 - p[P::Sharp]).powi(2) * v.w as f64 / (v.span * 360.0);
    if s < 0.3 { 0.0 } else { s }
}

#[derive(Script, ScriptHook)]
pub struct Post {
    #[live] pub draw_final: DrawSkyFinal, #[live] pub draw_blur: DrawSkyBlur, #[live] pub draw_water: DrawSkyWater, #[live] pub draw_water_day: DrawSkyWaterDay,
    /// The water and blur targets; what the last picture ran: the water, the blur's sigma (px, 0 none); the scene's margin (px).
    #[rust] pub water: Option<SceneTarget>, #[rust] pub blur: Option<SceneTarget>, #[rust] ran: (bool, f64), #[rust] margin: usize,
}

impl Post {
    /// Whether the scene list also runs mirrored (the reflections): the page's rule, some strength and some sea, or lakes;
    /// or a wet ground (`SkyLight::wet`), which mirrors at full strength (render.rs also asks for water in the view).
    pub fn wants_mirror(&self, p: &Params, light: &SkyLight) -> bool {
        (p[P::PrRefl] > 0.0 && (p[P::Land] < 0.999 || p[P::PrLake] > 0.0)) || light.wet > 0.0
    }

    /// The view to draw the scene at for `v` under `light`: widened by the blur's margin, so lights at its edges blur with
    /// their real neighbours as on the page, and by the water's when it reflects, so the ripple's sideways samples find the
    /// mirror there (tiles of an export meet without a seam); the final crops the margin off.
    pub fn scene_view(&mut self, p: &Params, v: &ViewSpec, light: &SkyLight) -> ViewSpec {
        let pad = if self.wants_mirror(p, light) { water_margin(p, v) } else { 0 };
        self.margin = blur_margin(p, v) + pad;
        ViewSpec { pad, ..widen(v, self.margin) }
    }

    /// The passes between the scene and the final, recorded when the picture changes: the water when the mirror ran, the
    /// horizontal blur when its sigma shows. Each is the parent of the passes it samples.
    pub fn draw_passes(&mut self, cx: &mut Cx2d, sc: &SceneCtx, out: &SceneOut) {
        let v = out.view;
        let full = Rect { pos: dvec2(0.0, 0.0), size: dvec2(v.w as f64, v.h as f64) };
        let (mut head, mut light) = (out.scene.pass.draw_pass_id(), out.scene.light.clone());
        if let Some(mirror) = out.mirror {
            // by day the water mirrors the scene's colour too (DrawSkyWaterDay); in full night only its lights, with the
            // night's own shader
            let t = self.water.get_or_insert_with(|| SceneTarget::new(cx, "skyline_water", false));
            let d: &mut DrawQuad = if sc.light.night_share() < 1.0 { &mut self.draw_water_day } else { &mut self.draw_water };
            sc.set_uniforms(cx, &mut d.draw_vars);
            d.draw_vars.set_uniform(cx, live_id!(mirror_horizon), &[v.mirrored().horizon as f32]);
            for (name, tex) in [(live_id!(scene_light), &light), (live_id!(mirror_light), &mirror.light), (live_id!(mirror_color), &mirror.color), (live_id!(sea_tex), sc.sea)] {
                bind_texture(cx, &mut d.draw_vars, name, tex);
            }
            t.record(cx, v.w, v.h, |cx| d.draw_abs(cx, full));
            for child in [head, mirror.pass.draw_pass_id()] { cx.attach_child_pass(child, t.pass.draw_pass_id(), None); }
            (head, light) = (t.pass.draw_pass_id(), t.light.clone());
        }
        let sigma = blur_sigma(sc.p, v);
        if sigma > 0.0 {
            let (t, d) = (self.blur.get_or_insert_with(|| SceneTarget::new(cx, "skyline_blur", false)), &mut self.draw_blur);
            bind_texture(cx, &mut d.draw_vars, live_id!(scene_light), &light);
            d.draw_vars.set_uniform(cx, live_id!(sigma), &[sigma as f32]);
            t.record(cx, v.w, v.h, |cx| d.draw_abs(cx, full));
            cx.attach_child_pass(head, t.pass.draw_pass_id(), None);
        }
        self.ran = (out.mirror.is_some(), sigma);
    }

    /// The final composite into `rect` of the current pass, every draw; it attaches the pass chain it samples to the
    /// current pass (so it paints first and stays alive).
    pub fn draw_final(&mut self, cx: &mut Cx2d, rect: Rect, out: &SceneOut, tone: bool) {
        let (water, sigma) = self.ran;
        let t = if sigma > 0.0 { self.blur.as_ref() } else if water { self.water.as_ref() } else { None };
        let (pass, light) = t.map_or((&out.scene.pass, &out.scene.light), |t| (&t.pass, &t.light));
        cx.make_child_pass(pass);
        let vars = &mut self.draw_final.draw_vars;
        for (name, tex) in [(live_id!(scene_color), &out.scene.color), (live_id!(scene_light), light)] { bind_texture(cx, vars, name, tex); }
        vars.set_uniform(cx, live_id!(tone), &[if tone { 1.0 } else { 0.0 }]);
        vars.set_uniform(cx, live_id!(sigma), &[sigma as f32]);
        let (m, w, h) = (self.margin as f32, out.view.w as f32, out.view.h as f32);
        vars.set_uniform(cx, live_id!(crop), &[m / w, m / h, 1.0 - 2.0 * m / w, 1.0 - 2.0 * m / h]);
        self.draw_final.draw_abs(cx, rect);
    }
}
