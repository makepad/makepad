use super::text_prepare::MpPreparedBrowserScene;
use super::{MpBrowserPictureId, MpBrowserScene, MpBrowserSceneExecState};
use crate::quad::{draw_composited_quad, evaluate_clip_chain, MpClipBasis, MpCompositedQuad};
use crate::scene::MpBackfaceVisibility;
use crate::*;

impl MpBrowserSceneExecState {
    pub(super) fn draw_picture(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &MpPreparedBrowserScene,
        picture_id: MpBrowserPictureId,
    ) {
        let Some(picture) = scene.pictures.get(picture_id) else {
            return;
        };
        let Some(texture) = self.render_task(cx, scene, prepared_text, picture.task_id) else {
            return;
        };
        let Some(transform) = scene
            .primitive_scene
            .transforms
            .get(picture.transform_id)
            .copied()
        else {
            return;
        };
        let Some(clip_chain) = scene.primitive_scene.clip_chains.get(picture.clip_chain_id) else {
            return;
        };
        let basis = MpClipBasis::from_cx(cx, transform.scene_from_origin);
        let evaluated = evaluate_clip_chain(clip_chain, &basis);
        let quad = MpCompositedQuad {
            texture,
            local_rect: picture.local_rect,
            uv_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(1.0, 1.0),
            },
            transform: transform.scene_from_origin,
            opacity: picture.opacity,
            premultiplied: true,
            backface_visible: matches!(transform.backface_visibility, MpBackfaceVisibility::Visible),
            depth_write: false,
            clip_planes: evaluated.clip_planes,
            mask: evaluated.masks,
        };
        draw_composited_quad(
            cx,
            &quad,
            &mut self.draw_quad,
            &mut self.draw_corner_quad,
            &mut self.draw_projective_quad,
        );
    }
}
