//! GPU plumbing for a MilkDrop frame: render-to-texture passes sized in
//! pixels, and the per-frame uniform and texture binding every shader
//! derived from `DrawMilkBase` shares.
//!
//! A [`Target`] is one `DrawPass` + `DrawList2d` + colour texture, begun and
//! ended inside a widget's draw (the same idiom as the VJ post chain).
//! makepad runs a pass before the pass it is attached to (its consumer), so
//! each target attaches the targets it reads that were rendered this frame:
//! the mesh → warp → blur → composite chain is one line of attachments, and
//! the host attaches the last target to the pass it draws in.

use crate::shaders::*;
use crate::state::FrameParams;
use makepad_widgets::*;

/// One render target and the pass that draws into it.
pub struct Target {
    pub pass: DrawPass,
    list: DrawList2d,
    pub texture: Texture,
    /// The size last rendered, in pixels.
    pub size: (usize, usize),
}

impl Target {
    pub fn new(cx: &mut Cx, name: &str, format: TextureFormat) -> Target {
        let pass = DrawPass::new_with_name(cx, name);
        let list = DrawList2d::new(cx);
        let texture = Texture::new_with_format(cx, format).label(cx, "milkdrop/target");
        pass.set_color_texture(cx, &texture, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)));
        Target { pass, list, texture, size: (0, 0) }
    }

    pub fn bgra(cx: &mut Cx, name: &str) -> Target {
        Target::new(cx, name, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true })
    }

    pub fn rgba_f32(cx: &mut Cx, name: &str) -> Target {
        Target::new(cx, name, TextureFormat::RenderRGBAf32 { size: TextureSize::Auto, initial: true })
    }

    /// Render into this target at `size` pixels; `draw` gets the full rect.
    /// `inputs` are the passes rendered this frame whose textures `draw`
    /// reads: they run first.
    pub fn run(
        &mut self,
        cx: &mut Cx2d,
        size: (usize, usize),
        inputs: &[&DrawPass],
        draw: &mut dyn FnMut(&mut Cx2d, Rect),
    ) {
        let size = (size.0.max(1), size.1.max(1));
        self.size = size;
        self.pass.set_size(cx, dvec2(size.0 as f64, size.1 as f64));
        // DPI 1: logical units are pixels.
        cx.begin_pass(&self.pass, Some(1.0));
        self.list.begin_always(cx);
        for input in inputs {
            cx.make_child_pass(input);
        }
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        draw(cx, Rect { pos: dvec2(0.0, 0.0), size: pass_size });
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }
}

/// The textures bound to the 15 slots of `DrawMilkBase` for one draw.
pub struct TextureSet<'a> {
    pub main: &'a Texture,
    pub mesh: &'a Texture,
    pub blur: [&'a Texture; 3],
    pub noise_lq: &'a Texture,
    pub noise_lq_lite: &'a Texture,
    pub noise_mq: &'a Texture,
    pub noise_hq: &'a Texture,
    pub noisevol_lq: &'a Texture,
    pub noisevol_hq: &'a Texture,
    pub images: [&'a Texture; 4],
}

pub fn bind_textures(dv: &mut DrawVars, t: &TextureSet) {
    dv.set_texture(TEX_MAIN, t.main);
    dv.set_texture(TEX_MESH, t.mesh);
    for (i, b) in t.blur.iter().enumerate() {
        dv.set_texture(TEX_BLUR1 + i, b);
    }
    dv.set_texture(TEX_NOISE_LQ, t.noise_lq);
    dv.set_texture(TEX_NOISE_LQ_LITE, t.noise_lq_lite);
    dv.set_texture(TEX_NOISE_MQ, t.noise_mq);
    dv.set_texture(TEX_NOISE_HQ, t.noise_hq);
    dv.set_texture(TEX_NOISEVOL_LQ, t.noisevol_lq);
    dv.set_texture(TEX_NOISEVOL_HQ, t.noisevol_hq);
    for (i, img) in t.images.iter().enumerate() {
        dv.set_texture(TEX_IMG0 + i, img);
    }
}

/// What every base-derived shader reads this frame.
pub struct FrameUniforms {
    pub time: f32,
    pub fps: f32,
    pub frame: f32,
    pub progress: f32,
    pub audio: [f32; 4],
    pub audio_att: [f32; 4],
    pub aspect: (f32, f32),
    pub mesh: (usize, usize),
    pub pixels: (usize, usize),
}

pub fn set_uniforms(dv: &mut DrawVars, cx: &Cx, u: &FrameUniforms, p: &FrameParams) {
    let (ax, ay) = u.aspect;
    dv.set_uniform(cx, live_id!(u_time), &[u.time, u.fps, u.frame, u.progress]);
    dv.set_uniform(cx, live_id!(u_audio), &u.audio);
    dv.set_uniform(cx, live_id!(u_audio_att), &u.audio_att);
    dv.set_uniform(cx, live_id!(u_aspect), &[ax, ay, 1.0 / ax.max(1e-6), 1.0 / ay.max(1e-6)]);
    const Q: [LiveId; 8] = [
        live_id!(u_q0),
        live_id!(u_q1),
        live_id!(u_q2),
        live_id!(u_q3),
        live_id!(u_q4),
        live_id!(u_q5),
        live_id!(u_q6),
        live_id!(u_q7),
    ];
    for (i, id) in Q.iter().enumerate() {
        dv.set_uniform(cx, *id, &p.q[i * 4..i * 4 + 4]);
    }
    dv.set_uniform(cx, live_id!(u_rand_frame), &p.rand_frame);
    dv.set_uniform(cx, live_id!(u_rand_preset), &p.rand_preset);
    dv.set_uniform(cx, live_id!(u_roam_cos), &p.roam_cos);
    dv.set_uniform(cx, live_id!(u_roam_sin), &p.roam_sin);
    dv.set_uniform(cx, live_id!(u_slow_roam_cos), &p.slow_roam_cos);
    dv.set_uniform(cx, live_id!(u_slow_roam_sin), &p.slow_roam_sin);
    dv.set_uniform(cx, live_id!(u_blur_min), &[p.blur_min[0], p.blur_min[1], p.blur_min[2], 0.0]);
    dv.set_uniform(cx, live_id!(u_blur_max), &[p.blur_max[0], p.blur_max[1], p.blur_max[2], 1.0]);
    let m = p.motion;
    dv.set_uniform(cx, live_id!(u_pf0), &[m.zoom, m.zoomexp, m.rot, m.warp]);
    dv.set_uniform(cx, live_id!(u_pf1), &[m.cx, m.cy, m.dx, m.dy]);
    dv.set_uniform(cx, live_id!(u_pf2), &[m.sx, m.sy, p.decay, if p.wrap { 1.0 } else { 0.0 }]);
    dv.set_uniform(cx, live_id!(u_warp_t), &[p.warp_time, p.warp_scale_inv, 0.0, 0.0]);
    dv.set_uniform(cx, live_id!(u_warp_f), &p.warp_f);
    let (gx, gy) = u.mesh;
    dv.set_uniform(cx, live_id!(u_mesh), &[gx as f32, gy as f32, (gx + 1) as f32, (gy + 1) as f32]);
    dv.set_uniform(cx, live_id!(u_pixels), &[u.pixels.0 as f32, u.pixels.1 as f32, 0.0, 0.0]);
}

/// The composite's own uniforms: MilkDrop 1 filters and the hue corners.
pub fn set_comp_uniforms(dv: &mut DrawVars, cx: &Cx, p: &FrameParams) {
    let b = |v: bool| if v { 1.0 } else { 0.0 };
    dv.set_uniform(cx, live_id!(u_comp0), &[p.echo_zoom, p.echo_alpha, p.echo_orient, p.gamma]);
    dv.set_uniform(cx, live_id!(u_comp1), &[b(p.brighten), b(p.darken), b(p.solarize), b(p.invert)]);
    dv.set_uniform(cx, live_id!(u_comp2), &[b(p.darken_center), 0.0, 0.0, 0.0]);
    const HUE: [LiveId; 4] = [live_id!(u_hue0), live_id!(u_hue1), live_id!(u_hue2), live_id!(u_hue3)];
    for (i, id) in HUE.iter().enumerate() {
        let h = p.hue[i];
        dv.set_uniform(cx, *id, &[h[0], h[1], h[2], 0.0]);
    }
}

/// A CPU texture of BGRA pixels (noise, stand-in images, 1×1 fills).
pub fn bgra_texture(cx: &mut Cx, width: usize, height: usize, data: Vec<u32>) -> Texture {
    Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 { width, height, data: Some(data), updated: TextureUpdated::Full },
    ).label(cx, "milkdrop/image")
}
