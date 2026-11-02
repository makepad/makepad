//! Still-view supersampling: a viewport that stops moving converges to an
//! anti-aliased picture over a few frames, then draws nothing more.
//!
//! A host that keeps its scene passes retained (records them only when what
//! they show changes) gets no anti-aliasing from that: every silhouette, crease
//! and outline is one hard pixel step, and it crawls when the view moves.
//! [`StillAa`] takes the frame the host already drew as sample 0, then asks
//! for [`StillAa::samples`]` - 1` more, each recorded through
//! [`jittered_projection`] (the same view moved by a sub-pixel offset of a
//! low-discrepancy sequence), and keeps their running average in an RGBA16F
//! target. Everything the host draws through that projection is averaged
//! together: geometry edges, ray-marched surfaces, shadow and AO noise,
//! selection outlines.
//!
//! The average is display-referred (what the host would have shown), so an
//! edge between a bright and a dark surface resolves after the tone curve,
//! the way a film scanner sees it.
//!
//! Costs: a still view pays `samples - 1` extra recordings once, then
//! nothing; a moving view pays nothing (the host shows its own frame and
//! calls [`StillAa::reset`] whenever it records for another reason).
//!
//! The blend pass is ping-pong (it reads the previous average and writes the
//! other target), so painting the same recording twice gives the same
//! result: a pass held back for a compiling pipeline and painted later cannot
//! count a sample twice.

use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The running average: prev + (src - prev) * u_blend.x, where u_blend.x is
    // 1 / (samples so far + 1). The first sample (weight 1) only copies.
    mod.draw.DrawStillAaBlend = mod.std.set_type_default() do #(DrawStillAaBlend::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        src: texture_2d(float)
        prev: texture_2d(float)
        u_blend: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        pixel: fn() -> vec4f {
            let s = self.src.sample_nearest(self.pos)
            if self.u_blend.x >= 1.0 {
                return s
            }
            let p = self.prev.sample_nearest(self.pos)
            return p + (s - p) * self.u_blend.x
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStillAaBlend {
    #[deref]
    pub draw_super: DrawQuad,
}

/// The samples a still view converges over (sample 0 is the host's own frame).
pub const STILL_AA_SAMPLES: u32 = 16;

/// The jitter's reach in pixels either side of the pixel centre: a little
/// over half a pixel, so the resolve is a slightly soft box rather than a
/// sharp one (edges stop stair-stepping without looking blurred).
const JITTER_REACH: f32 = 0.6;

fn halton(mut i: u32, base: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

/// Sample `k`'s offset from the pixel centre, in pixels (x right, y down).
/// Sample 0 is the centre (the frame the host drew without jitter); the rest
/// follow the (2, 3) Halton sequence over the jitter's reach.
pub fn still_aa_offset(k: u32) -> (f32, f32) {
    if k == 0 {
        return (0.0, 0.0);
    }
    let x = halton(k, 2) - 0.5;
    let y = halton(k, 3) - 0.5;
    (x * 2.0 * JITTER_REACH, y * 2.0 * JITTER_REACH)
}

/// `projection` moved by `offset_px` pixels on a target of `size_px` pixels:
/// a clip-space translation applied after the projection, so it holds for a
/// perspective and an orthographic camera alike (it scales with clip w).
pub fn jittered_projection(projection: Mat4f, offset_px: (f32, f32), size_px: (f32, f32)) -> Mat4f {
    if offset_px == (0.0, 0.0) || size_px.0 <= 0.0 || size_px.1 <= 0.0 {
        return projection;
    }
    let mut t = Mat4f::identity();
    t.v[12] = 2.0 * offset_px.0 / size_px.0;
    // pixel y runs down, clip y up
    t.v[13] = -2.0 * offset_px.1 / size_px.1;
    Mat4f::mul(&t, &projection)
}

/// The running average of a still view's jittered frames (see the module).
#[derive(Default)]
pub struct StillAa {
    pass: Option<(DrawPass, DrawList)>,
    targets: Option<[Texture; 2]>,
    draw: Option<Box<DrawStillAaBlend>>,
    /// Samples in the average held by `targets[cur]`.
    count: u32,
    cur: usize,
}

impl StillAa {
    /// Forget the average: the host recorded its scene for another reason (the
    /// view, the scene or a setting changed). It shows its own frame again
    /// until the next sample lands.
    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// Samples in the average.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The offset of the next sample to record, in pixels; `None` when the
    /// view has converged (or before the host drew its own frame: sample 0
    /// needs no recording, the host adds the frame it has).
    pub fn next_offset(&self) -> Option<(f32, f32)> {
        (self.count < STILL_AA_SAMPLES).then(|| still_aa_offset(self.count))
    }

    /// The averaged picture, once it holds more than the host's own frame.
    pub fn output(&self) -> Option<&Texture> {
        if self.count < 2 {
            return None;
        }
        self.targets.as_ref().map(|t| &t[self.cur])
    }

    /// For a draw that takes a sample: attach the blend pass under the pass
    /// being drawn and return it. The host then parents the producer of its
    /// frame under it (that draw only), so the frame paints before the blend
    /// reads it, and calls [`StillAa::add`]. `None` while the draw cannot be
    /// built yet (the script VM is busy): no sample this draw.
    pub fn attach(&mut self, cx: &mut Cx2d) -> Option<DrawPassId> {
        if !self.ensure(cx.cx) {
            return None;
        }
        let (pass, _) = self.pass.as_ref()?;
        cx.make_child_pass(pass);
        Some(pass.draw_pass_id())
    }

    fn ensure(&mut self, cx: &mut Cx) -> bool {
        if self.pass.is_none() {
            self.pass = Some((DrawPass::new_with_name(cx, "still aa"), DrawList::new(cx)));
        }
        if self.targets.is_none() {
            let t = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
            self.targets = Some([t(cx), t(cx)]);
        }
        if self.draw.is_none() {
            self.draw = cx.try_with_vm(|vm| Box::new(DrawStillAaBlend::script_new_with_default(vm)));
        }
        self.draw.is_some()
    }

    /// Add `src` (the host's frame for [`StillAa::next_offset`], `size` its
    /// logical size) to the average, in a draw that [`StillAa::attach`]ed the
    /// blend pass. False when there is nothing to add to.
    pub fn add(&mut self, cx: &mut Cx2d, src: &Texture, size: DVec2) -> bool {
        if self.count >= STILL_AA_SAMPLES || !self.ensure(cx.cx) {
            return false;
        }
        let targets = self.targets.clone().unwrap();
        let (read, write) = (targets[self.cur].clone(), targets[self.cur ^ 1].clone());
        let weight = 1.0 / (self.count + 1) as f32;
        let draw = self.draw.as_mut().unwrap();
        {
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_texture(0, src);
            dv.set_texture(1, &read);
            dv.set_uniform(cx.cx, live_id!(u_blend), &[weight, 0.0, 0.0, 0.0]);
        }
        let (pass, list) = self.pass.as_mut().unwrap();
        pass.set_size(cx, size);
        pass.clear_color_textures(cx.cx);
        pass.set_color_texture(cx, &write, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        pass.clear_depth_texture(cx.cx);
        cx.begin_pass(pass, None);
        pass.set_size(cx, size);
        list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
        cx.end_pass_sized_turtle();
        list.end(cx);
        cx.end_pass(pass);
        self.cur ^= 1;
        self.count += 1;
        true
    }
}
