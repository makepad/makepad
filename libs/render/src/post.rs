//! The HDR lane's post chain between the scene pass and its composite:
//! bloom and auto-exposure (see `Renderer::set_hdr_output`).
//!
//! [`BloomPass`] is a self-contained chain of offscreen passes, wired the way
//! [`crate::SsaoPass`] is: the host parents its scene pass under
//! [`BloomPass::first_pass_id`] and hands [`BloomPass::run`] the pass that
//! composites (the window), so the pass sort runs scene -> bloom -> composite.
//!
//! * **Bloom** — the energy-conserving "physically based" kind (Jimenez,
//!   Call of Duty AW): no threshold, a 13-tap downsample chain to ~16 px and
//!   a 3x3 tent upsample chain back, each level added to the one above. The
//!   composite mixes a few percent of it into the scene, so only what is
//!   genuinely bright (sun disc, lamps, emissive windows at night) visibly
//!   glows. The first downsample weights by 1/(1+luma) (Karis) so a single
//!   hot texel cannot flicker as a blob.
//! * **Auto-exposure** — the geometric mean luminance of the smallest bloom
//!   level, adapted over time into a 1x1 target (ping-pong: the adapt pass
//!   reads last frame's value). The composite divides a mid-grey key by it
//!   and clamps the result to a band around the host's metered exposure, so
//!   a dark interior opens up and a bright one closes down, but night stays
//!   night.
//!
//! Cost: at 1440p the chain is ~11 small full-screen draws, the first at
//! half resolution; each later one a quarter of the pixels of the one before.

use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // One level down: the 13-tap filter (four overlapping 2x2 boxes around
    // the centre plus the centre box), weighted 0.5 / 0.125 x4.
    mod.draw.DrawBloomDown = mod.std.set_type_default() do #(DrawBloomDown::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        src: texture_2d(float)
        // xy = one SOURCE texel in uv, z = 1 on the first level (Karis
        // weighting against fireflies), w unused.
        u_src: uniform(vec4(0.001, 0.001, 0.0, 0.0))
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }

        tap: fn(uv: vec2) -> vec3 {
            let c = max(self.src.sample(uv).xyz, vec3(0.0, 0.0, 0.0))
            if self.u_src.z > 0.5 {
                return c / (1.0 + dot(c, vec3(0.2126, 0.7152, 0.0722)))
            }
            return c
        }

        pixel: fn() -> vec4f {
            let uv = self.pos
            let t = self.u_src.xy
            let a = self.tap(uv + vec2(0.0 - 2.0 * t.x, 0.0 - 2.0 * t.y))
            let b = self.tap(uv + vec2(0.0, 0.0 - 2.0 * t.y))
            let c = self.tap(uv + vec2(2.0 * t.x, 0.0 - 2.0 * t.y))
            let d = self.tap(uv + vec2(0.0 - 2.0 * t.x, 0.0))
            let e = self.tap(uv)
            let f = self.tap(uv + vec2(2.0 * t.x, 0.0))
            let g = self.tap(uv + vec2(0.0 - 2.0 * t.x, 2.0 * t.y))
            let h = self.tap(uv + vec2(0.0, 2.0 * t.y))
            let i = self.tap(uv + vec2(2.0 * t.x, 2.0 * t.y))
            let j = self.tap(uv + vec2(0.0 - t.x, 0.0 - t.y))
            let k = self.tap(uv + vec2(t.x, 0.0 - t.y))
            let l = self.tap(uv + vec2(0.0 - t.x, t.y))
            let m = self.tap(uv + vec2(t.x, t.y))
            var o = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125
            if self.u_src.z > 0.5 {
                // Undo the Karis weight on the (now averaged) result.
                o = o / max(1.0 - dot(o, vec3(0.2126, 0.7152, 0.0722)), 0.05)
            }
            return vec4(o, 1.0)
        }
    }

    // One level up: 3x3 tent over the smaller level, added to this level's
    // own downsample.
    mod.draw.DrawBloomUp = mod.std.set_type_default() do #(DrawBloomUp::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        small: texture_2d(float)
        same: texture_2d(float)
        // xy = one SMALL-level texel in uv
        u_small: uniform(vec4(0.001, 0.001, 0.0, 0.0))
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }

        pixel: fn() -> vec4f {
            let uv = self.pos
            let t = self.u_small.xy
            var s = self.small.sample(uv).xyz * 4.0
            s = s + (self.small.sample(uv + vec2(t.x, 0.0)).xyz + self.small.sample(uv - vec2(t.x, 0.0)).xyz
                + self.small.sample(uv + vec2(0.0, t.y)).xyz + self.small.sample(uv - vec2(0.0, t.y)).xyz) * 2.0
            s = s + self.small.sample(uv + t).xyz + self.small.sample(uv - t).xyz
                + self.small.sample(uv + vec2(t.x, 0.0 - t.y)).xyz + self.small.sample(uv + vec2(0.0 - t.x, t.y)).xyz
            return vec4(self.same.sample(uv).xyz + s * 0.0625, 1.0)
        }
    }

    // 1x1: geometric-mean scene luminance from the smallest level, adapted
    // toward over time. r = adapted mean luminance.
    mod.draw.DrawExposureAdapt = mod.std.set_type_default() do #(DrawExposureAdapt::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        level: texture_2d(float)
        previous: texture_2d(float)
        // x = adaptation blend this frame (1 - exp(-dt * rate)), y = 1 when
        // `previous` holds a valid value, z/w unused.
        u_adapt: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }

        pixel: fn() -> vec4f {
            // 8x8 samples of log luminance, centre-weighted (the subject is
            // rarely in the corners).
            var sum = 0.0
            var weight = 0.0
            var y = 0.0
            while y < 8.0 {
                var x = 0.0
                while x < 8.0 {
                    let uv = vec2((x + 0.5) / 8.0, (y + 0.5) / 8.0)
                    let c = self.level.sample(uv).xyz
                    let raw = dot(c, vec3(0.2126, 0.7152, 0.0722))
                    // A non-finite texel (NaN from one bad shade, or an
                    // f16 overflow to Inf) is skipped: folded in, it made
                    // the mean — and so the whole frame, sky included —
                    // black. Both comparisons are false for NaN.
                    if raw >= 0.0 && raw < 60000.0 {
                        let l = max(raw, 0.0001)
                        let d = length(uv - vec2(0.5, 0.5))
                        let w = 1.0 - d * 0.9
                        sum = sum + log2(l) * w
                        weight = weight + w
                    }
                    x = x + 1.0
                }
                y = y + 1.0
            }
            var current = 0.18
            if weight > 0.0 {
                current = exp2(sum / weight)
            }
            let prev = self.previous.sample_nearest(vec2(0.5, 0.5)).x
            var adapted = current
            if self.u_adapt.y > 0.5 && prev > 0.0 && prev < 60000.0 {
                adapted = exp2(mix(log2(prev), log2(current), self.u_adapt.x))
            }
            return vec4(adapted, current, 0.0, 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBloomDown {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBloomUp {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawExposureAdapt {
    #[deref]
    pub draw_super: DrawQuad,
}

struct Stage {
    pass: DrawPass,
    list: DrawList,
}

/// Smallest bloom level edge, in pixels: the chain stops above it.
const MIN_LEVEL_PX: f64 = 16.0;
/// How fast the eye adapts: e-folding rate per second.
const ADAPT_RATE: f32 = 0.8;

/// The bloom + auto-exposure chain (see the module doc).
#[derive(Default)]
pub struct BloomPass {
    /// down[0..n], up[0..n-1] (up[i] has down[i]'s size), then the adapt pass.
    down: Vec<Stage>,
    up: Vec<Stage>,
    adapt: Option<Stage>,
    down_tex: Vec<Texture>,
    up_tex: Vec<Texture>,
    exposure_tex: [Option<Texture>; 2],
    /// Which exposure texture the adapt pass writes this frame.
    flip: usize,
    /// Frames the adapted exposure has been valid for.
    valid_frames: u32,
    draw_down: Option<Box<DrawBloomDown>>,
    draw_up: Option<Box<DrawBloomUp>>,
    draw_adapt: Option<Box<DrawExposureAdapt>>,
    last_time: Option<f64>,
    levels: usize,
    /// Total GPU ms of the chain (a frame or two behind).
    pub gpu_ms: f64,
}

impl BloomPass {
    fn stage(cx: &mut Cx, name: &str) -> Stage {
        let pass = DrawPass::new_with_name(cx, name);
        pass.set_gpu_timing_enabled(cx, std::env::var_os("TMP_POST_TIMING").is_some());
        Stage { pass, list: DrawList::new(cx) }
    }

    fn rgba16f(cx: &mut Cx) -> Texture {
        Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true })
    }

    /// Create the passes, targets and draw structs for `levels` bloom levels.
    /// False while the script VM is held elsewhere (try again next frame).
    fn ensure(&mut self, cx: &mut Cx, levels: usize) -> bool {
        while self.down.len() < levels {
            self.down.push(Self::stage(cx, "bloom down"));
            self.down_tex.push(Self::rgba16f(cx));
        }
        while self.up.len() + 1 < levels {
            self.up.push(Self::stage(cx, "bloom up"));
            self.up_tex.push(Self::rgba16f(cx));
        }
        if self.adapt.is_none() {
            self.adapt = Some(Self::stage(cx, "exposure adapt"));
        }
        for t in &mut self.exposure_tex {
            if t.is_none() {
                *t = Some(Self::rgba16f(cx));
            }
        }
        if self.draw_down.is_none() {
            self.draw_down = cx.try_with_vm(|vm| Box::new(DrawBloomDown::script_new_with_default(vm)));
        }
        if self.draw_up.is_none() {
            self.draw_up = cx.try_with_vm(|vm| Box::new(DrawBloomUp::script_new_with_default(vm)));
        }
        if self.draw_adapt.is_none() {
            self.draw_adapt = cx.try_with_vm(|vm| Box::new(DrawExposureAdapt::script_new_with_default(vm)));
        }
        self.draw_down.is_some() && self.draw_up.is_some() && self.draw_adapt.is_some()
    }

    /// The deepest pass of the chain: the host parents its scene pass under
    /// it so the scene renders first. None until the first `run`.
    pub fn first_pass_id(&self) -> Option<DrawPassId> {
        self.down.first().map(|s| s.pass.draw_pass_id())
    }

    /// Bloom levels in the chain (the composite normalises by it).
    pub fn levels(&self) -> usize {
        self.levels
    }

    /// The bloom texture (half resolution) the composite adds.
    pub fn bloom(&self) -> Option<&Texture> {
        if self.levels < 2 { self.down_tex.first() } else { self.up_tex.first() }
    }

    /// The adapted-luminance texture (1x1, r = adapted mean luminance, g =
    /// this frame's) and whether it holds a settled value yet.
    pub fn exposure(&self) -> (Option<&Texture>, bool) {
        (self.exposure_tex[self.flip].as_ref(), self.valid_frames > 1)
    }

    /// Record the chain for one frame. `scene` is the host's HDR scene
    /// target of logical `size`; `parent` is the pass that composites it.
    pub fn run(&mut self, cx: &mut Cx2d, size: DVec2, scene: &Texture, parent: DrawPassId) {
        let dpi = cx.current_dpi_factor();
        let px = size * dpi;
        let mut levels = 1usize;
        while levels < 8 && px.x.min(px.y) / (1u32 << (levels + 1)) as f64 >= MIN_LEVEL_PX {
            levels += 1;
        }
        if !self.ensure(cx.cx, levels) {
            return;
        }
        if levels != self.levels {
            self.levels = levels;
        }
        let now = cx.time();
        let dt = self.last_time.map_or(0.0, |t| (now - t).clamp(0.0, 0.5)) as f32;
        self.last_time = Some(now);

        // The chain order, deepest first: down0 .. down[n-1], up[n-2] .. up0,
        // adapt, then the host's composite pass.
        let mut order: Vec<DrawPassId> = Vec::new();
        for s in self.down.iter().take(levels) {
            order.push(s.pass.draw_pass_id());
        }
        for s in self.up.iter().take(levels - 1).rev() {
            order.push(s.pass.draw_pass_id());
        }
        order.push(self.adapt.as_ref().unwrap().pass.draw_pass_id());
        for w in order.windows(2) {
            cx.cx.passes[w[0]].parent = CxDrawPassParent::DrawPass(w[1]);
        }
        if let Some(last) = order.last() {
            cx.cx.passes[*last].parent = CxDrawPassParent::DrawPass(parent);
        }

        fn record(cx: &mut Cx2d, stage: &mut Stage, size: DVec2, target: &Texture, draw: &mut dyn FnMut(&mut Cx2d, Rect)) {
            let id = stage.pass.draw_pass_id();
            let parent = cx.cx.passes[id].parent.clone();
            stage.pass.set_size(cx, size);
            stage.pass.clear_color_textures(cx.cx);
            stage.pass.set_color_texture(cx, target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)));
            cx.cx.passes[id].depth_texture = None;
            cx.cx.passes[id].parent = parent;
            cx.begin_pass(&stage.pass, Some(1.0));
            stage.pass.set_size(cx, size);
            stage.list.begin_always(cx);
            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            draw(cx, Rect { pos: dvec2(0.0, 0.0), size });
            cx.end_pass_sized_turtle();
            stage.list.end(cx);
            cx.end_pass(&stage.pass);
        }

        // Level sizes in PIXELS (the passes run at dpi 1).
        let level_size = |i: usize| {
            let s = (1u32 << (i + 1)) as f64;
            dvec2((px.x / s).floor().max(1.0), (px.y / s).floor().max(1.0))
        };

        // ---- down ----------------------------------------------------------
        for i in 0..levels {
            let (src, src_px) = if i == 0 { (scene.clone(), px) } else { (self.down_tex[i - 1].clone(), level_size(i - 1)) };
            let dst = self.down_tex[i].clone();
            let draw = self.draw_down.as_mut().unwrap();
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_texture(0, &src);
            dv.set_uniform(cx.cx, live_id!(u_src), &[1.0 / src_px.x as f32, 1.0 / src_px.y as f32, if i == 0 { 1.0 } else { 0.0 }, 0.0]);
            record(cx, &mut self.down[i], level_size(i), &dst, &mut |cx, r| draw.draw_abs(cx, r));
        }
        // ---- up --------------------------------------------------------------
        for i in (0..levels.saturating_sub(1)).rev() {
            let small = if i + 2 == levels { self.down_tex[i + 1].clone() } else { self.up_tex[i + 1].clone() };
            let same = self.down_tex[i].clone();
            let dst = self.up_tex[i].clone();
            let small_px = level_size(i + 1);
            let draw = self.draw_up.as_mut().unwrap();
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_texture(0, &small);
            dv.set_texture(1, &same);
            dv.set_uniform(cx.cx, live_id!(u_small), &[1.0 / small_px.x as f32, 1.0 / small_px.y as f32, 0.0, 0.0]);
            record(cx, &mut self.up[i], level_size(i), &dst, &mut |cx, r| draw.draw_abs(cx, r));
        }
        // ---- adapt -------------------------------------------------------------
        {
            let read = self.flip;
            self.flip ^= 1;
            let previous = self.exposure_tex[read].clone().unwrap();
            let target = self.exposure_tex[self.flip].clone().unwrap();
            let smallest = self.down_tex[levels - 1].clone();
            let draw = self.draw_adapt.as_mut().unwrap();
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_texture(0, &smallest);
            dv.set_texture(1, &previous);
            let blend = 1.0 - (-dt * ADAPT_RATE).exp();
            dv.set_uniform(cx.cx, live_id!(u_adapt), &[blend, if self.valid_frames > 0 { 1.0 } else { 0.0 }, 0.0, 0.0]);
            record(cx, self.adapt.as_mut().unwrap(), dvec2(1.0, 1.0), &target, &mut |cx, r| draw.draw_abs(cx, r));
            self.valid_frames = self.valid_frames.saturating_add(1);
        }
        // ---- timing ------------------------------------------------------------
        let mut total = 0.0;
        let mut any = false;
        for s in self.down.iter().chain(self.up.iter()).chain(self.adapt.iter()) {
            if let Some(ms) = s.pass.take_gpu_times_ms(cx.cx).last() {
                total += ms;
                any = true;
            }
        }
        if any {
            self.gpu_ms = total;
        }
    }
}
