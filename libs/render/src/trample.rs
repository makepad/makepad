//! Trampled grass: a field of where the ground was pressed (tyres, feet),
//! drawn on the GPU and read by the grass blades at their roots
//! (shaders/grass.rs), so a car leaves a flattened trail through a meadow
//! and a walker a path that stands up again over [`RECOVERY`] seconds.
//!
//! One world-anchored square per renderer (not per eye), centred on the
//! host's focus, its corner snapped to whole texels: a scroll is a whole
//! texel offset, and the feedback pass reads the previous target there with
//! point reads, so moving never blurs it. Eight bits a channel (the decay
//! below counts byte levels): R, G the way the blades were pushed (128 at
//! rest), B how flat they lie.
//!
//! The decay is linear in byte levels and exact at any frame rate: a clock
//! in levels advances `255 * dt / RECOVERY` a frame and each texel steps
//! where the clock plus its own phase crosses a whole level, so over any
//! span every texel loses the span's levels to within one (a multiplicative
//! decay, or a whole-level step, stalls at 120 Hz: 0.27 levels a frame).
//!
//! Stamps are the host's, every frame, from its contacts (a vehicle's
//! grounded wheels, a character's feet): one instanced quad each, no cap.
//! Cost: nothing while no stamp has landed for [`RECOVERY`] seconds (the
//! pass is not recorded and the blades skip the read); otherwise one pass of
//! `texels^2` point reads plus the stamps' quads.

use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The previous target, moved by the scroll and eased toward rest.
    mod.draw.DrawTrampleFeedback = mod.std.set_type_default() do #(DrawTrampleFeedback::script_shader(vm)){
        ..mod.draw.DrawQuad,
        color_format: @Bgra8NoBlend
        alpha_blend: false
        previous: texture_2d(float)
        // x, y: this target's texel (i, j) is the previous one's (i + x, j + y);
        // z: 1 when the previous target holds the field; w: texels a side.
        trample_scroll: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        // x: the decay clock's fraction of a level, y: levels this frame,
        // z, w: the corner's world texel, modulo 4096.
        trample_decay: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }
        pixel: fn() -> vec4 {
            let n = self.trample_scroll.w
            let texel = floor(self.pos * n)
            let src = texel + self.trample_scroll.xy
            if self.trample_scroll.z < 0.5 || src.x < 0.0 || src.y < 0.0 || src.x >= n || src.y >= n {
                return vec4(128.0 / 255.0, 128.0 / 255.0, 0.0, 1.0)
            }
            let v = floor(self.previous.sample_nearest((src + vec2(0.5, 0.5)) / n) * 255.0 + vec4(0.5, 0.5, 0.5, 0.5))
            // The texel's own phase (interleaved gradient noise of its world
            // texel), so the field does not step all at once.
            let wt = texel + self.trample_decay.zw
            let w = wt - floor(wt / 4096.0) * 4096.0
            let phase = fract(52.9829189 * fract(dot(w, vec2(0.06711056, 0.00583715))))
            let base = self.trample_decay.x + phase
            let d = floor(base + self.trample_decay.y) - floor(base)
            let push = v.xy - vec2(128.0, 128.0)
            let eased = sign(push) * max(abs(push) - vec2(d, d), vec2(0.0, 0.0))
            return vec4((eased.x + 128.0) / 255.0, (eased.y + 128.0) / 255.0, max(v.z - d, 0.0) / 255.0, 1.0)
        }
        fragment: fn() { self.fb0 = self.pixel() }
    }

    // One stamp: an ellipse on the field, blended over it (premultiplied):
    // the field moves `w` of the way to the stamp's push and flatness, all
    // the way inside its core, so it lands whole in the frame it is drawn.
    mod.draw.DrawTrampleStamp = mod.std.set_type_default() do #(DrawTrampleStamp::script_shader(vm)){
        ..mod.draw.DrawQuad,
        // Texels a side.
        trample_texels: uniform(1.0)
        v_local: varying(vec2f)
        v_off: varying(vec2f)
        vertex: fn() {
            // The quad covers the ellipse and a texel more each way.
            let reach = self.stamp_half + vec2(1.0, 1.0)
            let l = (self.geom.pos * 2.0 - vec2(1.0, 1.0)) * reach
            let along = self.stamp_axis
            let across = vec2(along.y, 0.0 - along.x)
            let off = across * l.x + along * l.y
            let at = (self.stamp_centre + off) / self.trample_texels
            self.v_local = l / max(self.stamp_half, vec2(0.0001, 0.0001))
            self.v_off = off
            self.pos = at
            self.world = vec4(at.x, at.y, 0.0, 1.0)
            self.vertex_pos = vec4(at.x * 2.0 - 1.0, 1.0 - at.y * 2.0, 0.0, 1.0)
        }
        pixel: fn() -> vec4 {
            let r = length(self.v_local)
            let w = 1.0 - smoothstep(0.55, 1.0, r)
            // Its own way, and out from under its middle: the blades splay.
            let away = self.v_off / max(length(self.v_off), 0.0001)
            let p = self.stamp_push + away * (0.45 * min(r, 1.0))
            let push = p / max(length(p), 1.0)
            let rg = (vec2(128.0, 128.0) + push * 127.0) / 255.0
            return vec4(rg.x * w, rg.y * w, self.stamp_strength * w, w)
        }
        fragment: fn() { self.fb0 = self.pixel() }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTrampleFeedback {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTrampleStamp {
    #[deref]
    pub draw_super: DrawQuad,
    /// Its middle, in the target's texels.
    #[live(vec2(0.0, 0.0))]
    pub stamp_centre: Vec2f,
    /// Half its width and length, texels.
    #[live(vec2(1.0, 1.0))]
    pub stamp_half: Vec2f,
    /// The way its length runs (unit, target axes).
    #[live(vec2(0.0, 1.0))]
    pub stamp_axis: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub stamp_push: Vec2f,
    #[live(0.0)]
    pub stamp_strength: f32,
}

/// Seconds a blade pressed flat takes to stand up again.
pub const RECOVERY: f64 = 8.0;
/// Byte levels: the field holds push and flatness as bytes.
const LEVELS: f64 = 255.0;

/// One contact pressing the ground this frame: an ellipse on the ground
/// plane (the field is a plane: a host sends contacts with the ground).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroundStamp {
    /// World x and z of its middle.
    pub pos: Vec2f,
    /// Half its width and half its length, metres.
    pub half_extents: Vec2f,
    /// Its length runs along world (x, z) = (sin yaw, cos yaw).
    pub yaw: f32,
    /// The way it pushes the blades over, world (x, z), length up to 1.
    pub push: Vec2f,
    /// How flat it presses them, 0..1.
    pub strength: f32,
}

/// What a tier spends on the field: its side, metres and texels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrampleBudget {
    pub extent: f32,
    pub texels: usize,
}

struct TrampleGpu {
    budget: TrampleBudget,
    targets: [Texture; 2],
    pass: DrawPass,
    list: DrawList,
    feedback: Box<DrawTrampleFeedback>,
    stamp: Box<DrawTrampleStamp>,
}

/// The field (see the module). The renderer owns one; the host feeds it
/// through `Renderer::set_ground_stamps`.
#[derive(Default)]
pub struct TrampleField {
    budget: Option<TrampleBudget>,
    gpu: Option<TrampleGpu>,
    /// The latest call's contacts and focus (world x, z), and the game
    /// seconds since the field last ran.
    stamps: Vec<GroundStamp>,
    focus: [f64; 2],
    dt: f64,
    pending: bool,
    /// The current target's corner, world texels.
    origin: [i64; 2],
    cur: usize,
    /// The current target holds the field.
    valid: bool,
    /// Levels decayed since the last frame with a stamp: from LEVELS + 1 on
    /// every texel is at rest.
    quiet: f64,
    /// The decay clock, levels.
    clock: f64,
    told_stamp: bool,
    told_dt: bool,
    told_focus: bool,
    /// A review camera's copy of the live field's read (its own never runs).
    mirrored: Option<(Texture, [f32; 4])>,
}

impl TrampleField {
    pub fn new(budget: Option<TrampleBudget>) -> Self {
        Self { budget, quiet: f64::INFINITY, ..Self::default() }
    }

    /// The tier's field, or none (Low): the next run rebuilds the targets.
    pub fn set_budget(&mut self, budget: Option<TrampleBudget>) {
        self.budget = budget;
    }

    /// This frame's contacts (those `keep` accepts), the field's centre and
    /// the game seconds since the last call; the field runs at the next
    /// scene draw. Calls between two draws: the last contacts, every call's
    /// seconds. A stamp with a value that is not finite is left out and a
    /// negative or non-finite `dt` counts as 0, each said once in the log.
    pub fn set_stamps(&mut self, focus: Vec3f, dt: f32, stamps: &[GroundStamp], keep: impl Fn(&GroundStamp) -> bool) {
        let dt = if dt.is_finite() && dt >= 0.0 {
            dt
        } else {
            if !self.told_dt {
                self.told_dt = true;
                log!("trample: a frame's seconds were {dt}; counted as 0");
            }
            0.0
        };
        self.stamps.clear();
        for s in stamps {
            let finite = [s.pos.x, s.pos.y, s.half_extents.x, s.half_extents.y, s.yaw, s.push.x, s.push.y, s.strength].iter().all(|v| v.is_finite());
            if !finite {
                if !self.told_stamp {
                    self.told_stamp = true;
                    log!("trample: a stamp with a value that is not finite was left out ({s:?})");
                }
                continue;
            }
            if keep(s) {
                self.stamps.push(*s);
            }
        }
        if focus.x.is_finite() && focus.z.is_finite() {
            self.focus = [focus.x as f64, focus.z as f64];
        } else if !self.told_focus {
            self.told_focus = true;
            log!("trample: a focus that is not finite ({focus:?}); the field stays where it was");
        }
        self.dt += dt as f64;
        self.pending = true;
    }

    /// Run the field once after `set_stamps`: a child pass of the current
    /// pass (the scene's), so it is drawn before the scene reads it.
    pub fn record(&mut self, cx: &mut CxDraw) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        let dt = std::mem::take(&mut self.dt);
        let Some(budget) = self.budget.filter(|b| b.texels >= 2 && b.extent > 0.0 && b.extent.is_finite()) else {
            self.release(cx.cx);
            return;
        };
        // Every texel at rest and nothing new: no pass.
        if self.stamps.is_empty() && (!self.valid || self.quiet >= LEVELS + 1.0) {
            return;
        }
        if self.gpu.as_ref().is_some_and(|g| g.budget != budget) {
            self.release(cx.cx);
        }
        if self.gpu.is_none() {
            let Some((feedback, stamp)) = cx.try_with_vm(|vm| {
                (Box::new(DrawTrampleFeedback::script_new_with_default(vm)), Box::new(DrawTrampleStamp::script_new_with_default(vm)))
            }) else {
                return;
            };
            let n = budget.texels;
            let target = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: n, height: n }, initial: true });
            self.gpu = Some(TrampleGpu {
                budget,
                targets: [target(cx.cx), target(cx.cx)],
                pass: DrawPass::new_with_name(cx.cx, "trample"),
                list: DrawList::new(cx.cx),
                feedback,
                stamp,
            });
            self.valid = false;
        }
        let g = self.gpu.as_mut().unwrap();
        // While its pipeline compiles the pass draws nothing: the field
        // starts at rest once it can.
        let ready = g.feedback.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready(id, false));
        let n = budget.texels;
        let texel = budget.extent as f64 / n as f64;
        let half = (n / 2) as i64;
        let origin = [(self.focus[0] / texel).floor() as i64 - half, (self.focus[1] / texel).floor() as i64 - half];
        let scroll = [origin[0] - self.origin[0], origin[1] - self.origin[1]];
        let carried = self.valid && scroll.iter().all(|s| s.unsigned_abs() < n as u64);
        let step = (dt * LEVELS / RECOVERY).min(4.0 * LEVELS);
        let previous = self.cur;
        self.cur ^= 1;
        let size = dvec2(n as f64, n as f64);
        let dv = &mut g.feedback.draw_vars;
        dv.set_uniform(cx, live_id!(trample_scroll), &[scroll[0] as f32, scroll[1] as f32, if carried { 1.0 } else { 0.0 }, n as f32]);
        dv.set_uniform(cx, live_id!(trample_decay), &[self.clock.fract() as f32, step as f32, origin[0].rem_euclid(4096) as f32, origin[1].rem_euclid(4096) as f32]);
        dv.set_texture(0, &g.targets[previous]);
        g.stamp.draw_vars.set_uniform(cx, live_id!(trample_texels), &[n as f32]);
        let mut cx2 = Cx2d::new(cx);
        cx2.make_child_pass(&g.pass);
        cx2.begin_pass(&g.pass, Some(1.0));
        g.pass.set_size(cx2.cx, size);
        g.pass.clear_color_textures(cx2.cx);
        g.pass.set_color_texture(cx2.cx, &g.targets[self.cur], DrawPassClearColor::ClearWith(vec4(128.0 / 255.0, 128.0 / 255.0, 0.0, 1.0)));
        g.list.begin_always(&mut cx2);
        cx2.begin_root_turtle(size, Layout::flow_overlay());
        g.feedback.draw_abs(&mut cx2, Rect { pos: dvec2(0.0, 0.0), size });
        for s in &self.stamps {
            let st = &mut g.stamp;
            st.stamp_centre = vec2((s.pos.x as f64 / texel - origin[0] as f64) as f32, (s.pos.y as f64 / texel - origin[1] as f64) as f32);
            // A contact narrower than a texel still presses the one it is on.
            st.stamp_half = vec2((s.half_extents.x.abs() as f64 / texel).max(1.0) as f32, (s.half_extents.y.abs() as f64 / texel).max(1.0) as f32);
            st.stamp_axis = vec2(s.yaw.sin(), s.yaw.cos());
            st.stamp_push = s.push / s.push.length().max(1.0);
            st.stamp_strength = s.strength.clamp(0.0, 1.0);
            st.draw_abs(&mut cx2, Rect { pos: dvec2(0.0, 0.0), size });
        }
        cx2.end_pass_sized_turtle();
        g.list.end(&mut cx2);
        cx2.end_pass(&g.pass);
        self.origin = origin;
        self.clock += step;
        self.quiet = if self.stamps.is_empty() { self.quiet + step } else { 0.0 };
        self.valid = ready;
    }

    /// What the blades read: the current target and (corner x, z, 1 / side,
    /// 1), or None while every texel is at rest.
    pub fn grass_read(&self) -> Option<(&Texture, [f32; 4])> {
        if let Some((texture, read)) = &self.mirrored {
            return Some((texture, *read));
        }
        let g = self.gpu.as_ref()?;
        if !self.valid || self.quiet >= LEVELS + 1.0 || self.budget != Some(g.budget) {
            return None;
        }
        let texel = g.budget.extent as f64 / g.budget.texels as f64;
        Some((&g.targets[self.cur], [(self.origin[0] as f64 * texel) as f32, (self.origin[1] as f64 * texel) as f32, 1.0 / g.budget.extent, 1.0]))
    }

    /// A review camera (a fork of the live renderer) shows the live field
    /// as it is this frame.
    pub fn mirror_from(&mut self, live: &TrampleField) {
        self.mirrored = live.grass_read().map(|(texture, read)| (texture.clone(), read));
    }

    fn release(&mut self, cx: &mut Cx) {
        if let Some(g) = self.gpu.take() {
            g.pass.clear_color_textures(cx);
            g.pass.withdraw(cx);
        }
        self.valid = false;
    }
}

#[cfg(test)]
#[path = "trample/gpu_guard.rs"]
mod gpu_guard;
