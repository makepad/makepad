//! THE NV12 PRESENT PASS: the texture-to-texture stage that turns a
//! player's resident NV12 frame into the RGBA slot texture the rest of
//! the GPU pipeline (mixer, fx, program out) consumes. The CPU's only job
//! is two plane memcpys into the Y (R8) and UV (RG8) textures; the YUV→RGB
//! arithmetic runs in this pass's pixel shader — the operator's law:
//! never unpack 4K video in a software loop, the core just pins.
//!
//! Same offscreen recipe as [`crate::flow_warp::FlowWarpView`]: a 4×4
//! widget whose child DrawPass renders at exact video resolution into a
//! RenderBGRAu8 texture that replaces the slot's decoder texture upstream.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawNv12::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_y: texture_2d(float)
        tex_uv: texture_2d(float)

        // This quad fills its own offscreen pass. The stock DrawQuad
        // vertex clamps against the PARENT (window) clip and would slice
        // the pass to the on-screen widget region — transform in pure
        // pass space instead (the flow-warp recipe).
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        // Biplanar NV12, BT.709 LIMITED range — the exact math the CPU
        // converter spoke (and widgets/video.rs speaks for camera video).
        pixel: fn() {
            let yv = self.tex_y.sample(self.pos).x
            let uv = self.tex_uv.sample(self.pos).xy
            let y = (yv * 255.0 - 16.0) / 219.0
            let u = (uv.x * 255.0 - 128.0) / 224.0
            let v = (uv.y * 255.0 - 128.0) / 224.0
            let r = y + 1.5748 * v
            let g = y - 0.1873 * u - 0.4681 * v
            let b = y + 1.8556 * u
            return vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0)
        }
    }

    mod.widgets.Nv12ViewBase = #(Nv12View::register_widget(vm))
    mod.widgets.Nv12View = set_type_default() do mod.widgets.Nv12ViewBase{
        width: 4
        height: 4
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNv12 {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct Nv12View {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_nv12: DrawNv12,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[rust]
    area: Area,
    /// Resolution the render target was created at ((0,0) = not yet). The
    /// target is FIXED-size so Image-hosted consumers can measure it.
    #[rust]
    target_size: (u32, u32),
    /// Y (R8, w×h) and UV (RG8, w/2×h/2) plane textures, recreated only
    /// on a resolution change.
    #[rust]
    planes: Option<(Texture, Texture)>,
    #[rust]
    size: (u32, u32),
    /// At least one pass has rendered — the output texture is real.
    #[rust]
    rendered: bool,
}

impl Nv12View {
    /// Upload one NV12 frame's planes. Two contiguous memcpys — the Y
    /// plane is the first w×h bytes, the interleaved UV plane maps 1:1
    /// onto an RG8 texture at half resolution.
    pub fn set_frame(&mut self, cx: &mut Cx, data: &[u8], width: u32, height: u32) {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || data.len() < w * h * 3 / 2 {
            return;
        }
        if self.size != (width, height) || self.planes.is_none() {
            self.planes = Some((
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecRu8 {
                        width: w,
                        height: h,
                        data: Some(vec![0; w * h]),
                        unpack_row_length: None,
                        updated: TextureUpdated::Full,
                    },
                ),
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecRGu8 {
                        width: w / 2,
                        height: h / 2,
                        data: Some(vec![0; (w / 2) * (h / 2) * 2]),
                        unpack_row_length: None,
                        updated: TextureUpdated::Full,
                    },
                ),
            ));
            self.size = (width, height);
            self.rendered = false;
        }
        let (y_tex, uv_tex) = self.planes.as_ref().unwrap();
        let mut buf = y_tex.take_vec_u8(cx);
        buf.clear();
        buf.extend_from_slice(&data[..w * h]);
        y_tex.put_back_vec_u8(cx, buf, None);
        let mut buf = uv_tex.take_vec_u8(cx);
        buf.clear();
        buf.extend_from_slice(&data[w * h..w * h + (w / 2) * (h / 2) * 2]);
        uv_tex.put_back_vec_u8(cx, buf, None);
        self.area.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.planes = None;
        self.size = (0, 0);
        self.target_size = (0, 0);
        self.rendered = false;
        self.area.redraw(cx);
    }

    /// The pass output — only once a pass has actually rendered into it
    /// (an unrendered target composites as black).
    pub fn output_texture(&self) -> Option<Texture> {
        if self.rendered {
            Some(self.color_texture.clone())
        } else {
            None
        }
    }

    fn ensure_target(&mut self, cx: &mut Cx) {
        if self.target_size == self.size {
            return;
        }
        self.target_size = self.size;
        self.rendered = false;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: self.size.0 as usize,
                    height: self.size.1 as usize,
                },
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
    }
}

impl WidgetNode for Nv12View {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for Nv12View {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.walk_turtle_with_area(&mut self.area, walk);
        let Some((y_tex, uv_tex)) = self.planes.clone() else {
            return DrawStep::done();
        };
        self.ensure_target(cx.cx);
        let (w, h) = self.size;
        self.draw_nv12.draw_vars.set_texture(0, &y_tex);
        self.draw_nv12.draw_vars.set_texture(1, &uv_tex);
        // Child pass at exact video resolution, dpi locked to 1 (the
        // thumbnail-renderer recipe: re-assert the size after begin_pass
        // or the texture takes the window's rect).
        let size = dvec2(w as f64, h as f64);
        self.pass.set_size(cx, size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.pass.set_size(cx, size);
        self.pass.set_dpi_factor(cx, 1.0);
        self.draw_list.begin_always(cx);
        self.draw_nv12.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);
        self.rendered = true;
        DrawStep::done()
    }
}
