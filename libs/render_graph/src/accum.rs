//! Locked-time sub-frame accumulation: motion blur by averaging sub-frames
//! rendered at their canonical times, with an adaptive count
//! (PDOOM-PARITY R1, KERNELS.md §3.4.2).
//!
//! The host renders each sub-frame (every layer of it: 3D, lines, text, 2D)
//! into one linear f16 target and hands it to [`Accumulator::add`], which
//! adds it into a full-float running sum. A pixel that is not finite (a
//! stray NaN from one shader in one sub-frame of hundreds) is dropped, or it
//! would poison the average and bloom into a disc. After each adaptive step
//! the accumulator measures how far the displayed frame moves when the
//! step's new sub-frames join the old ones: per block of 2x2 logical pixels,
//! the difference of the two averages through a display proxy (an
//! exponential shoulder at the frame's exposure, then sRGB) in 8-bit
//! levels, reduced to its worst block on the GPU and read back (a few
//! bytes). [`crate::mode::Schedule`] decides from it whether to go on.
//!
//! One sub-frame is rendered per host paint (a pass never samples a
//! texture a later pass of the same paint writes, and every sub-frame
//! reuses the host's scene passes). The host drives it:
//!
//! ```ignore
//! acc.begin(Sampling::Adaptive(AdaptiveSampling::default()));
//! // every paint:
//! match acc.step(cx) {
//!     Step::Render { u, .. } => { render the frame at t + u·shutter into `sub`; acc.add(cx, &sub, size, None) }
//!     Step::Wait => { keep the chain attached, redraw again }
//!     Step::Finish { .. } => { let (avg, first) = acc.finish(cx, size, None); post from `avg` }
//!     Step::Idle => {}
//! }
//! ```

use crate::mode::{Next, Sampling, Schedule, Sum};
use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawGraphAccumBase = mod.std.set_type_default() do #(DrawGraphAccumBase::script_shader(vm)){
        ..mod.draw.DrawQuad
        depth_clip: 0.0
        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(self.geom.pos.x * 2.0 - 1.0, 1.0 - self.geom.pos.y * 2.0, 0.0, 1.0)
        }
    }

    // sum' = sum + sub, dropping non-finite pixels. u.x = 1 when `prev`
    // holds a sum (else it is the first sub-frame of the sum).
    mod.draw.DrawGraphAccum = mod.std.set_type_default() do #(DrawGraphAccum::script_shader(vm)){
        ..mod.draw.DrawGraphAccumBase
        color_format: @Rgba32F
        sub: texture_2d(float)
        prev: texture_2d(float)
        u_acc: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        pixel: fn() -> vec4f {
            let c = self.sub.sample_nearest(self.pos)
            let finite = abs(c.x) <= 60000.0 && abs(c.y) <= 60000.0 && abs(c.z) <= 60000.0 && abs(c.w) <= 60000.0
            var s = vec4(0.0, 0.0, 0.0, 0.0)
            if self.u_acc.x > 0.5 {
                s = self.prev.sample_nearest(self.pos)
            }
            if finite {
                return s + c
            }
            return s
        }
    }

    // sum' = a + b (a step's new sub-frames join the converged ones).
    mod.draw.DrawGraphMerge = mod.std.set_type_default() do #(DrawGraphMerge::script_shader(vm)){
        ..mod.draw.DrawGraphAccumBase
        color_format: @Rgba32F
        a: texture_2d(float)
        b: texture_2d(float)
        pixel: fn() -> vec4f {
            return self.a.sample_nearest(self.pos) + self.b.sample_nearest(self.pos)
        }
    }

    // The average: sum / n, linear half float.
    mod.draw.DrawGraphAverage = mod.std.set_type_default() do #(DrawGraphAverage::script_shader(vm)){
        ..mod.draw.DrawGraphAccumBase
        color_format: @Rgba16F
        sum: texture_2d(float)
        u_avg: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        pixel: fn() -> vec4f {
            return self.sum.sample_nearest(self.pos) * self.u_avg.x
        }
    }

    // Per block of B x B source pixels: how far the displayed value moves
    // between the average of sum a (x 1/n) and of sum b (x 1/2n), in 8-bit
    // levels (the worst channel). u_err: 1/n, 1/2n, B, exposure;
    // u_src: source size in px (xy), one source texel in uv (zw).
    mod.draw.DrawGraphError = mod.std.set_type_default() do #(DrawGraphError::script_shader(vm)){
        ..mod.draw.DrawGraphAccumBase
        color_format: @Rgba16F
        a: texture_2d(float)
        b: texture_2d(float)
        u_err: uniform(vec4(1.0, 0.5, 2.0, 1.0))
        u_src: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        disp: fn(x: vec3) -> vec3 {
            let y = vec3(1.0, 1.0, 1.0) - exp(max(x, vec3(0.0, 0.0, 0.0)) * (0.0 - self.u_err.w))
            let s = clamp(y, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            return mix(s * 12.92, pow(s, vec3(1.0 / 2.4, 1.0 / 2.4, 1.0 / 2.4)) * 1.055 - vec3(0.055, 0.055, 0.055), step(vec3(0.0031308, 0.0031308, 0.0031308), s))
        }
        pixel: fn() -> vec4f {
            let b = self.u_err.z
            let p0 = floor(self.pos * self.u_src.xy / b) * b
            var sa = vec3(0.0, 0.0, 0.0)
            var sb = vec3(0.0, 0.0, 0.0)
            var y = 0.0
            while y < b {
                var x = 0.0
                while x < b {
                    let p = min(p0 + vec2(x + 0.5, y + 0.5), self.u_src.xy - vec2(0.5, 0.5))
                    let uv = p * self.u_src.zw
                    sa = sa + self.a.sample_nearest(uv).xyz
                    sb = sb + self.b.sample_nearest(uv).xyz
                    x = x + 1.0
                }
                y = y + 1.0
            }
            let k = 1.0 / (b * b)
            let e = abs(self.disp(sa * (self.u_err.x * k)) - self.disp(sb * (self.u_err.y * k)))
            let m = max(e.x, max(e.y, e.z)) * 255.0
            // NaN reads as the worst error (never as converged).
            if m >= 0.0 {
                return vec4(m, 0.0, 0.0, 1.0)
            }
            return vec4(64.0, 0.0, 0.0, 1.0)
        }
    }

    // The worst error of 16 x 16 error texels, as a byte (levels x 4).
    // u_max: error size in texels (xy), one error texel in uv (zw).
    mod.draw.DrawGraphErrorMax = mod.std.set_type_default() do #(DrawGraphErrorMax::script_shader(vm)){
        ..mod.draw.DrawGraphAccumBase
        color_format: @Bgra8NoBlend
        err: texture_2d(float)
        u_max: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        pixel: fn() -> vec4f {
            let p0 = floor(self.pos * ceil(self.u_max.xy / 16.0)) * 16.0
            var m = 0.0
            var y = 0.0
            while y < 16.0 {
                var x = 0.0
                while x < 16.0 {
                    let p = p0 + vec2(x + 0.5, y + 0.5)
                    if p.x < self.u_max.x && p.y < self.u_max.y {
                        m = max(m, self.err.sample_nearest(p * self.u_max.zw).x)
                    }
                    x = x + 1.0
                }
                y = y + 1.0
            }
            let v = min(m * 4.0, 255.0) / 255.0
            return vec4(v, v, v, 1.0)
        }
    }
}

macro_rules! quad_draw {
    ($($name:ident),*) => {$(
        #[derive(Script, ScriptHook)]
        #[repr(C)]
        pub struct $name {
            #[deref]
            pub draw_super: DrawQuad,
        }
    )*};
}
quad_draw!(DrawGraphAccumBase, DrawGraphAccum, DrawGraphMerge, DrawGraphAverage, DrawGraphError, DrawGraphErrorMax);

/// What the host does this paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Render sub-frame `k` at shutter offset `u` (-0.5..0.5) and
    /// [`Accumulator::add`] it. `tap` is the supersampling tap (None when
    /// taps do not cycle).
    Render { k: u32, u: f32, tap: Option<u32> },
    /// A measurement is being read back: record nothing, draw again.
    Wait,
    /// Every sub-frame is summed: [`Accumulator::finish`], then post.
    Finish { n: u32 },
    /// No frame in progress.
    Idle,
}

struct Slot {
    pass: DrawPass,
    list: DrawList,
}

struct Draws {
    accum: Box<DrawGraphAccum>,
    merge: Box<DrawGraphMerge>,
    average: Box<DrawGraphAverage>,
    error: Box<DrawGraphError>,
    max: Box<DrawGraphErrorMax>,
}

/// The sub-frame accumulator of one host view.
#[derive(Default)]
pub struct Accumulator {
    schedule: Option<Schedule>,
    /// The schedule's step for the coming paint.
    upcoming: Option<Next>,
    draws: Option<Draws>,
    slots: Vec<Slot>,
    base: Option<[Texture; 2]>,
    new: Option<[Texture; 2]>,
    base_cur: usize,
    base_valid: bool,
    new_cur: usize,
    new_valid: bool,
    err: Option<Texture>,
    max: Option<Texture>,
    avg: Option<Texture>,
    ticket: Option<ReadbackTicket>,
    /// The slots the last recording used, in chain order.
    last_chain: Vec<usize>,
    /// The frame's exposure for the display proxy of the error estimate.
    pub exposure: f32,
    /// A block of the error estimate, in pixels: 2 logical pixels of a
    /// 720-line frame (2 at 720p, 3 at 1080p, 6 at 2160p).
    pub block_px: u32,
    /// Sub-frames of the last finished frame and its error after each step.
    pub last_count: u32,
    pub last_errors: Vec<f32>,
    /// Set when readback is unavailable (the count is then the minimum).
    pub readback_error: Option<String>,
}

impl Accumulator {
    /// Start a frame.
    pub fn begin(&mut self, sampling: Sampling) {
        let mut s = Schedule::new(sampling);
        self.upcoming = Some(s.next());
        self.schedule = Some(s);
        self.base_valid = false;
        self.new_valid = false;
        self.ticket = None;
        if self.exposure <= 0.0 {
            self.exposure = 1.0;
        }
        if self.block_px == 0 {
            self.block_px = 2;
        }
    }

    /// The shutter offsets of this frame (all that may be rendered).
    pub fn offsets(&self) -> &[f32] {
        self.schedule.as_ref().map_or(&[], |s| s.offsets())
    }

    pub fn in_progress(&self) -> bool {
        self.schedule.is_some()
    }

    /// What to do this paint. Polls a pending measurement.
    pub fn step(&mut self, cx: &mut Cx) -> Step {
        if let Some(ticket) = self.ticket {
            let Some(r) = cx.try_take_texture_readbacks_for(&[ticket]).into_iter().next() else {
                return Step::Wait;
            };
            self.ticket = None;
            let levels = match &r.data {
                Ok(bytes) => {
                    let mut m = 0u8;
                    for row in 0..r.height {
                        let line = &bytes[row * r.width * 4..(row + 1) * r.width * 4];
                        m = m.max(line.iter().enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, b)| *b).max().unwrap_or(0));
                    }
                    m as f32 / 4.0
                }
                Err(e) => {
                    self.readback_error = Some(e.to_string());
                    0.0
                }
            };
            if let Some(s) = &mut self.schedule {
                s.measured(levels);
                self.upcoming = Some(s.next());
            }
        }
        match self.upcoming {
            None => Step::Idle,
            Some(Next::Render { k, u, tap, .. }) => Step::Render { k, u, tap },
            Some(Next::Measure { .. }) => Step::Wait,
            Some(Next::Done { n }) => Step::Finish { n },
        }
    }

    fn ensure(&mut self, cx: &mut Cx) -> bool {
        if self.draws.is_none() {
            self.draws = cx.try_with_vm(|vm| Draws {
                accum: Box::new(DrawGraphAccum::script_new_with_default(vm)),
                merge: Box::new(DrawGraphMerge::script_new_with_default(vm)),
                average: Box::new(DrawGraphAverage::script_new_with_default(vm)),
                error: Box::new(DrawGraphError::script_new_with_default(vm)),
                max: Box::new(DrawGraphErrorMax::script_new_with_default(vm)),
            });
        }
        let f32t = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderRGBAf32 { size: TextureSize::Auto, initial: true });
        let f16t = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
        if self.base.is_none() {
            self.base = Some([f32t(cx), f32t(cx)]);
            self.new = Some([f32t(cx), f32t(cx)]);
            self.err = Some(f16t(cx));
            self.avg = Some(f16t(cx));
            self.max = Some(Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true }));
        }
        while self.slots.len() < 5 {
            let pass = DrawPass::new_with_name(cx, "graph accumulate");
            self.slots.push(Slot { pass, list: DrawList::new(cx) });
        }
        self.draws.is_some()
    }

    /// GPU bytes the accumulator holds at `size` (for admission).
    pub fn bytes(size: (u32, u32)) -> u64 {
        let px = size.0 as u64 * size.1 as u64;
        px * 16 * 4 + px * 8
    }

    /// Add this paint's sub-frame (the host rendered it into `sub`, linear
    /// f16, `size` pixels). Records the accumulate pass and, when a step
    /// completes, the measurement. Returns the first recorded pass: the
    /// host's passes producing `sub` parent under it. `parent` is the pass
    /// the chain ends in (None: a child of the pass being drawn).
    pub fn add(&mut self, cx: &mut Cx2d, sub: &Texture, size: (u32, u32), parent: Option<DrawPassId>) -> Option<DrawPassId> {
        let into = match self.upcoming {
            Some(Next::Render { into, .. }) => into,
            _ => return None,
        };
        if !self.ensure(cx.cx) {
            return None;
        }
        let dsize = dvec2(size.0 as f64, size.1 as f64);
        let mut chain: Vec<usize> = vec![0];
        // ---- accumulate ------------------------------------------------------
        let (pair, cur, valid) = match into {
            Sum::Base => (self.base.clone().unwrap(), &mut self.base_cur, &mut self.base_valid),
            Sum::New => (self.new.clone().unwrap(), &mut self.new_cur, &mut self.new_valid),
        };
        let (read, write) = (pair[*cur].clone(), pair[*cur ^ 1].clone());
        let had = *valid;
        *cur ^= 1;
        *valid = true;
        {
            let d = &mut self.draws.as_mut().unwrap().accum;
            let dv = &mut d.draw_super.draw_vars;
            dv.set_texture(0, sub);
            dv.set_texture(1, &read);
            dv.set_uniform(cx.cx, live_id!(u_acc), &[if had { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);
        }
        record(cx, &mut self.slots[0], dsize, &write, &mut self.draws.as_mut().unwrap().accum.draw_super);
        // ---- what comes next ------------------------------------------------
        let next = self.schedule.as_mut().map(|s| s.next());
        self.upcoming = next;
        if let Some(Next::Measure { n }) = next {
            self.record_measure(cx, size, n);
            chain.extend([1, 2, 3]);
            match self.max.as_ref().unwrap().read_back(cx.cx, ReadbackRequest { next_render: true }) {
                Ok(t) => self.ticket = Some(t),
                Err(e) => {
                    // No readback on this backend: the minimum count.
                    self.readback_error = Some(e.to_string());
                    if let Some(s) = &mut self.schedule {
                        s.measured(0.0);
                        self.upcoming = Some(s.next());
                    }
                }
            }
        }
        let ids: Vec<DrawPassId> = chain.iter().map(|&i| self.slots[i].pass.draw_pass_id()).collect();
        for w in ids.windows(2) {
            cx.cx.attach_child_pass(w[0], w[1], None);
        }
        attach(cx, &self.slots[*chain.last().unwrap()].pass, parent);
        self.last_chain = chain;
        Some(ids[0])
    }

    fn record_measure(&mut self, cx: &mut Cx2d, size: (u32, u32), n: u32) {
        let base = self.base.clone().unwrap();
        let new = self.new.clone().unwrap();
        let (a, b) = (base[self.base_cur].clone(), new[self.new_cur].clone());
        let block = self.block_px.max(1);
        let esize = (size.0.div_ceil(block), size.1.div_ceil(block));
        let msize = (esize.0.div_ceil(16), esize.1.div_ceil(16));
        let err = self.err.clone().unwrap();
        let max = self.max.clone().unwrap();
        let exposure = self.exposure;
        let d = self.draws.as_mut().unwrap();
        {
            let dv = &mut d.error.draw_super.draw_vars;
            dv.set_texture(0, &a);
            dv.set_texture(1, &b);
            dv.set_uniform(cx.cx, live_id!(u_err), &[1.0 / n as f32, 1.0 / (2 * n) as f32, block as f32, exposure]);
            dv.set_uniform(cx.cx, live_id!(u_src), &[size.0 as f32, size.1 as f32, 1.0 / size.0 as f32, 1.0 / size.1 as f32]);
        }
        record(cx, &mut self.slots[1], dvec2(esize.0 as f64, esize.1 as f64), &err, &mut d.error.draw_super);
        {
            let dv = &mut d.max.draw_super.draw_vars;
            dv.set_texture(0, &err);
            dv.set_uniform(cx.cx, live_id!(u_max), &[esize.0 as f32, esize.1 as f32, 1.0 / esize.0 as f32, 1.0 / esize.1 as f32]);
        }
        record(cx, &mut self.slots[2], dvec2(msize.0 as f64, msize.1 as f64), &max, &mut d.max.draw_super);
        // Merge: Base += New.
        let write = base[self.base_cur ^ 1].clone();
        {
            let dv = &mut d.merge.draw_super.draw_vars;
            dv.set_texture(0, &a);
            dv.set_texture(1, &b);
        }
        record(cx, &mut self.slots[3], dvec2(size.0 as f64, size.1 as f64), &write, &mut d.merge.draw_super);
        self.base_cur ^= 1;
        self.new_valid = false;
    }

    /// Record the average of the finished frame (call on [`Step::Finish`]).
    /// Returns the linear f16 average and the recorded pass (which parents
    /// under `parent`); the frame is over after this.
    pub fn finish(&mut self, cx: &mut Cx2d, size: (u32, u32), parent: Option<DrawPassId>) -> Option<(Texture, DrawPassId)> {
        let Some(Next::Done { n }) = self.upcoming else { return None };
        if !self.ensure(cx.cx) || !self.base_valid {
            return None;
        }
        let sum = self.base.as_ref().unwrap()[self.base_cur].clone();
        let avg = self.avg.clone().unwrap();
        {
            let dv = &mut self.draws.as_mut().unwrap().average.draw_super.draw_vars;
            dv.set_texture(0, &sum);
            dv.set_uniform(cx.cx, live_id!(u_avg), &[1.0 / n.max(1) as f32, 0.0, 0.0, 0.0]);
        }
        record(cx, &mut self.slots[4], dvec2(size.0 as f64, size.1 as f64), &avg, &mut self.draws.as_mut().unwrap().average.draw_super);
        let id = self.slots[4].pass.draw_pass_id();
        attach(cx, &self.slots[4].pass, parent);
        self.last_chain = vec![4];
        if let Some(s) = self.schedule.take() {
            self.last_count = s.count();
            self.last_errors = s.errors.clone();
        }
        self.upcoming = None;
        Some((avg, id))
    }

    /// The passes the last [`Accumulator::add`] recorded (for a host's
    /// painted check).
    pub fn recorded_ids(&self) -> Vec<DrawPassId> {
        self.last_chain.iter().map(|&i| self.slots[i].pass.draw_pass_id()).collect()
    }

    /// Every pass object the accumulator records with (for a host that
    /// re-attaches by id).
    pub fn pass_ids(&self) -> Vec<DrawPassId> {
        self.slots.iter().map(|s| s.pass.draw_pass_id()).collect()
    }

    /// One of the accumulator's passes by id.
    pub fn draw_pass(&self, id: DrawPassId) -> Option<&DrawPass> {
        self.slots.iter().map(|s| &s.pass).find(|p| p.draw_pass_id() == id)
    }

    /// The last pass of the last recorded chain (the host keeps it
    /// attached while it paints).
    pub fn last_pass(&self) -> Option<&DrawPass> {
        self.last_chain.last().map(|&i| &self.slots[i].pass)
    }
}

/// End a chain in `parent`, or as a child of the pass being drawn. Every
/// link inside a chain goes through `attach_child_pass(.., None)`: a pass
/// object that was a host's child in an earlier paint keeps that list as
/// its `attached_by`, and would be orphaned (not painted) once the list is
/// recorded again without it.
pub(crate) fn attach(cx: &mut Cx2d, pass: &DrawPass, parent: Option<DrawPassId>) {
    match parent {
        Some(p) => cx.cx.attach_child_pass(pass.draw_pass_id(), p, None),
        None => cx.make_child_pass(pass),
    }
}

fn record(cx: &mut Cx2d, slot: &mut Slot, size: DVec2, target: &Texture, draw: &mut DrawQuad) {
    slot.pass.set_size(cx, size);
    slot.pass.clear_color_textures(cx.cx);
    slot.pass.set_color_texture(cx, target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
    slot.pass.clear_depth_texture(cx.cx);
    cx.begin_pass(&slot.pass, Some(1.0));
    slot.pass.set_size(cx, size);
    slot.list.begin_always(cx);
    let pass_size = cx.current_pass_size();
    cx.begin_root_turtle(pass_size, Layout::flow_overlay());
    draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
    cx.end_pass_sized_turtle();
    slot.list.end(cx);
    cx.end_pass(&slot.pass);
}
