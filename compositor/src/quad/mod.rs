use crate::*;

mod clip;
mod draw2d;
mod draw3d;
mod shaders;
mod types;

pub(crate) use self::clip::{
    evaluate_clip_chain, evaluate_clip_chain_no_rect, set_clip_masks_evaluated,
    set_clip_planes_evaluated, MpClipBasis,
};
pub(crate) use self::draw2d::draw_composited_quad;
pub(crate) use self::shaders::{
    DrawMaskedProjectiveQuad, DrawProjectedCornerQuad, DrawProjectedQuad,
};
pub(crate) use self::types::{
    make_3d_batch, make_3d_island, make_3d_quad, MpCompositedQuad, MpInternal3dIsland,
};
pub use self::clip::MP_MAX_CLIP_PLANES;

pub(crate) fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    self::shaders::script_mod(vm)
}

pub(crate) struct MpCompositor {
    browser_primitives: crate::browser_primitives::MpBrowserPrimitiveRenderer,
    browser_scene_state: crate::browser_scene::MpBrowserSceneExecState,
    draw_quad: DrawProjectedQuad,
    draw_corner_quad: DrawProjectedCornerQuad,
    draw_projective_quad: DrawMaskedProjectiveQuad,
}

impl MpCompositor {
    pub(crate) fn new(cx: &mut Cx) -> Self {
        cx.with_vm(|vm| {
            makepad_draw::script_mod(vm);
            crate::script_mod(vm);
            Self {
                browser_primitives: crate::browser_primitives::MpBrowserPrimitiveRenderer::new(vm),
                browser_scene_state: crate::browser_scene::MpBrowserSceneExecState::new(vm),
                draw_quad: DrawProjectedQuad::script_new_with_default(vm),
                draw_corner_quad: DrawProjectedCornerQuad::script_new_with_default(vm),
                draw_projective_quad: DrawMaskedProjectiveQuad::script_new_with_default(vm),
            }
        })
    }

    pub(crate) fn draw_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &crate::scene::MpScene,
    ) -> Result<(), crate::scene::MpProjectError> {
        let (flat_quads, islands) = scene.partition_for_execution()?;
        for quad in &flat_quads {
            self.draw_internal_quad(cx, quad);
        }
        let batch = make_3d_batch(islands);
        if !batch.islands.is_empty() {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_internal_3d_batch(cx3d, &batch);
        }
        Ok(())
    }

    pub(crate) fn draw_browser_primitive_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &crate::browser_primitives::MpBrowserPrimitiveScene,
    ) {
        self.browser_primitives.draw_scene(cx, scene)
    }

    pub(crate) fn draw_browser_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &crate::browser_scene::MpBrowserScene,
    ) {
        self.browser_scene_state.draw_scene(cx, scene)
    }

    pub(crate) fn draw_retained_browser_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &crate::browser_scene::MpBrowserScene,
        prepared_text: &mut crate::browser_scene::MpPreparedBrowserScene,
    ) {
        self.browser_scene_state
            .draw_retained_scene(cx, scene, prepared_text)
    }

    pub(crate) fn browser_scene_frame_stats(
        &self,
    ) -> crate::browser_scene::MpBrowserSceneFrameStats {
        self.browser_scene_state.frame_stats()
    }

    pub(crate) fn draw_internal_quad(&mut self, cx: &mut Cx2d, quad: &MpCompositedQuad) {
        draw_composited_quad(
            cx,
            quad,
            &mut self.draw_quad,
            &mut self.draw_corner_quad,
            &mut self.draw_projective_quad,
        );
    }
}
