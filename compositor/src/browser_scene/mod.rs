use std::collections::HashMap;

use crate::browser_primitives::MpBrowserPrimitiveRenderer;
use crate::quad::{DrawMaskedProjectiveQuad, DrawProjectedCornerQuad, DrawProjectedQuad};
use crate::*;

mod fonts;
mod pictures;
mod scene_exec;
mod task_texture;
mod tasks;
mod text;
mod text_prepare;
mod types;

pub use self::types::{
    MpBrowserCacheKey, MpBrowserFontResource, MpBrowserGlyphInstance, MpBrowserPicture,
    MpBrowserPictureId, MpBrowserRetainedSceneId, MpBrowserScene, MpBrowserSceneFrameStats,
    MpBrowserSceneItem, MpBrowserStableTextRunId, MpBrowserTask, MpBrowserTaskId,
    MpBrowserTaskKind, MpBrowserTextDecorations, MpBrowserTextMetrics, MpBrowserTextRun,
    MpBrowserTextRunId, MpBrowserTextShadow,
};
use self::task_texture::DrawBrowserTaskTexture;
use self::tasks::TaskSurface;
pub use self::text_prepare::{MpPreparedBrowserScene, MpPreparedBrowserTextRunKey};
use self::text_prepare::BrowserTextCache;

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    self::task_texture::script_mod(vm)
}

pub(crate) struct MpBrowserSceneExecState {
    primitive_renderer: MpBrowserPrimitiveRenderer,
    draw_quad: DrawProjectedQuad,
    draw_corner_quad: DrawProjectedCornerQuad,
    draw_projective_quad: DrawMaskedProjectiveQuad,
    draw_task_texture: DrawBrowserTaskTexture,
    draw_color: DrawColor,
    draw_text: DrawText,
    text_cache: BrowserTextCache,
    scratch_surfaces: Vec<Option<TaskSurface>>,
    task_cache: HashMap<MpBrowserCacheKey, TaskSurface>,
    scratch_cursor: usize,
    frame_stats: MpBrowserSceneFrameStats,
}

impl MpBrowserSceneExecState {
    pub(crate) fn new(vm: &mut ScriptVm) -> Self {
        Self {
            primitive_renderer: MpBrowserPrimitiveRenderer::new(vm),
            draw_quad: DrawProjectedQuad::script_new_with_default(vm),
            draw_corner_quad: DrawProjectedCornerQuad::script_new_with_default(vm),
            draw_projective_quad: DrawMaskedProjectiveQuad::script_new_with_default(vm),
            draw_task_texture: DrawBrowserTaskTexture::script_new_with_default(vm),
            draw_color: DrawColor::script_new_with_default(vm),
            draw_text: DrawText::script_new_with_default(vm),
            text_cache: BrowserTextCache::new(),
            scratch_surfaces: Vec::new(),
            task_cache: HashMap::new(),
            scratch_cursor: 0,
            frame_stats: MpBrowserSceneFrameStats::default(),
        }
    }

    pub(crate) fn draw_scene(&mut self, cx: &mut Cx2d, scene: &MpBrowserScene) {
        let mut prepared_text = MpPreparedBrowserScene::new(scene.retained_scene_id);
        self.draw_retained_scene(cx, scene, &mut prepared_text);
    }

    pub(crate) fn draw_retained_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &mut MpPreparedBrowserScene,
    ) {
        self.begin_frame();
        self.text_cache.prepare_retained_scene(cx, scene, prepared_text);
        let text_stats = self.text_cache.frame_stats();
        self.frame_stats.prepared_text_batch_hit_count = text_stats.prepared_text_batch_hit_count;
        self.frame_stats.prepared_text_batch_miss_count = text_stats.prepared_text_batch_miss_count;
        self.frame_stats.prepared_text_batch_rebuild_count = text_stats.prepared_text_batch_rebuild_count;
        self.frame_stats.glyph_residency_hit_count = text_stats.glyph_residency_hit_count;
        self.frame_stats.glyph_residency_miss_count = text_stats.glyph_residency_miss_count;
        self.frame_stats.glyph_cache_reset_count = text_stats.glyph_cache_reset_count;
        self.frame_stats.atlas_page_alloc_count = text_stats.atlas_page_alloc_count;
        self.frame_stats.msdf_request_queue_count = text_stats.msdf_request_queue_count;
        self.frame_stats.msdf_completion_count = text_stats.msdf_completion_count;
        self.frame_stats.synchronous_fallback_glyph_generation_count =
            text_stats.synchronous_fallback_glyph_generation_count;
        self.draw_scene_inner(cx, scene, prepared_text);
    }

    pub(crate) fn frame_stats(&self) -> MpBrowserSceneFrameStats {
        self.frame_stats
    }

    fn begin_frame(&mut self) {
        self.scratch_cursor = 0;
        self.frame_stats = MpBrowserSceneFrameStats::default();
    }
}
