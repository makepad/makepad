use super::text_prepare::MpPreparedBrowserScene;
use super::{MpBrowserScene, MpBrowserSceneExecState, MpBrowserSceneItem};
use crate::*;

impl MpBrowserSceneExecState {
    pub(super) fn draw_scene_inner(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &MpPreparedBrowserScene,
    ) {
        for item in &scene.draw_order {
            match *item {
                MpBrowserSceneItem::PrimitiveBatch(batch_id) => {
                    self.primitive_renderer
                        .draw_batch(cx, &scene.primitive_scene, batch_id);
                }
                MpBrowserSceneItem::TextRun(text_run_id) => {
                    self.draw_text_run(cx, scene, prepared_text, text_run_id);
                }
                MpBrowserSceneItem::Picture(picture_id) => {
                    self.draw_picture(cx, scene, prepared_text, picture_id);
                }
            }
        }
    }
}
