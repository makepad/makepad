//! Shared Gaussian render passes for windows and layered compositors.
use crate::{
    gauss_chain::GaussChain,
    gauss_view::{GaussBlurSnapshot, GAUSS_VIEW_LEVELS},
    makepad_draw::*,
    window::{DrawGaussDownsample, DrawGaussScene, DrawGaussUpsample},
};
const GAUSS_STACK_LEVELS: usize = GAUSS_VIEW_LEVELS;
pub(crate) struct GaussStack {
    scene_pass: DrawPass,
    scene_draw_list: DrawList2d,
    scene_texture: Texture,
    _scene_depth_texture: Texture,
    chain: GaussChain,
}

pub(crate) fn gauss_render_texture_y_flip_for_os(os_type: &OsType) -> f32 {
    match os_type {
        OsType::Android(_) => 1.0,
        _ => 0.0,
    }
}

impl GaussStack {
    pub(crate) fn new(cx: &mut Cx) -> Self {
        let scene_pass = DrawPass::new_with_name(cx, "gauss_scene");
        let scene_draw_list = DrawList2d::new(cx);
        let scene_texture = Self::new_render_texture(cx);
        let scene_depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        scene_pass.set_color_texture(
            cx,
            &scene_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)),
        );
        scene_pass.set_depth_texture(cx, &scene_depth_texture, DrawPassClearDepth::ClearWith(1.0));
        scene_pass.set_live_with_parent(cx, true);

        let chain = GaussChain::new_for_stack(cx);

        Self {
            scene_pass,
            scene_draw_list,
            scene_texture,
            _scene_depth_texture: scene_depth_texture,
            chain,
        }
    }

    fn new_render_texture(cx: &mut Cx) -> Texture {
        Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        )
    }

    pub(crate) fn begin_scene(&mut self, cx: &mut Cx2d) {
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, None);
        self.scene_draw_list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
    }

    pub(crate) fn begin_scene_sized(&mut self, cx: &mut Cx2d, size: Vec2d) {
        let dpi = cx.current_dpi_factor();
        self.scene_pass.set_size(cx, size);
        cx.make_child_pass(&self.scene_pass);
        cx.begin_pass(&self.scene_pass, Some(dpi));
        self.scene_draw_list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
    }

    /// Passes in producer-before-consumer order. Layered compositors explicitly
    /// link this chain, so recycled pool IDs cannot reorder texture dependencies.
    pub(crate) fn dependencies(&self, count: usize) -> Vec<DrawPassId> {
        let mut passes = vec![self.scene_pass.draw_pass_id()];
        passes.extend(self.chain.dependencies(count));
        passes
    }

    pub(crate) fn end_scene(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.scene_draw_list.end(cx);
        cx.end_pass(&self.scene_pass);
    }

    pub(crate) fn snapshot(
        &self,
        root_size: Vec2d,
        source_y_flip: f32,
        dpi_factor: f64,
    ) -> GaussBlurSnapshot {
        GaussBlurSnapshot {
            scene_texture: self.scene_texture.clone(),
            mip_textures: self.chain.mip_textures(),
            source_size: root_size,
            source_y_flip,
            dpi_factor,
        }
    }

    pub(crate) fn draw_mip_chain(
        &mut self,
        cx: &mut Cx2d,
        downsample: &mut DrawGaussDownsample,
        root_size: Vec2d,
    ) {
        self.draw_mip_chain_to(cx, downsample, root_size, GAUSS_STACK_LEVELS);
    }

    pub(crate) fn draw_mip_chain_to(
        &mut self,
        cx: &mut Cx2d,
        downsample: &mut DrawGaussDownsample,
        root_size: Vec2d,
        count: usize,
    ) {
        self.chain
            .draw_mip_chain_to(cx, downsample, &self.scene_texture, root_size, count);
    }

    pub(crate) fn draw_high_blur_chain(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
    ) {
        self.draw_high_blur_chain_to(cx, upsample, root_size, GAUSS_STACK_LEVELS);
    }

    pub(crate) fn draw_high_blur_chain_to(
        &mut self,
        cx: &mut Cx2d,
        upsample: &mut DrawGaussUpsample,
        root_size: Vec2d,
        count: usize,
    ) {
        self.chain
            .draw_high_blur_chain_to(cx, upsample, root_size, count);
    }

    pub(crate) fn draw_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &mut DrawGaussScene,
        root_size: Vec2d,
    ) {
        self.draw_scene_region(
            cx,
            scene,
            root_size,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: root_size,
            },
        );
    }

    pub(crate) fn draw_scene_region(
        &mut self,
        cx: &mut Cx2d,
        scene: &mut DrawGaussScene,
        root_size: Vec2d,
        rect: Rect,
    ) {
        scene.draw_vars.set_uniform(
            cx,
            live_id!(source_offset),
            &[
                (rect.pos.x / root_size.x) as f32,
                (rect.pos.y / root_size.y) as f32,
            ],
        );
        scene.draw_vars.set_uniform(
            cx,
            live_id!(source_scale),
            &[
                (rect.size.x / root_size.x) as f32,
                (rect.size.y / root_size.y) as f32,
            ],
        );
        let source_y_flip = gauss_render_texture_y_flip_for_os(cx.os_type());
        scene
            .draw_vars
            .set_uniform(cx, live_id!(source_y_flip), &[source_y_flip]);
        scene.draw_vars.set_texture(0, &self.scene_texture);
        scene.draw_abs(cx, rect);
    }
}
