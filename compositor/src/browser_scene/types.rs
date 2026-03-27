use std::sync::Arc;

use crate::browser_primitives::{
    MpBrowserPrimitive, MpBrowserPrimitiveScene, MpPrimitiveBatchId, MpPrimitiveClipChain,
    MpPrimitiveClipChainId, MpPrimitiveTransform, MpPrimitiveTransformId,
};
use crate::scene::MpBlendMode;
use crate::*;

pub type MpBrowserTaskId = usize;
pub type MpBrowserPictureId = usize;
pub type MpBrowserTextRunId = usize;
pub type MpBrowserStableTextRunId = u64;
pub type MpBrowserRetainedSceneId = u64;
pub type MpBrowserCacheKey = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MpBrowserTextMetrics {
    pub advance_width_px: f32,
    pub baseline_ascent_px: f32,
    pub underline_offset_px: f32,
    pub underline_thickness_px: f32,
    pub strikeout_offset_px: f32,
    pub strikeout_thickness_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MpBrowserTextShadow {
    pub offset: DVec2,
    pub blur_radius_px: f32,
    pub color: Vec4f,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MpBrowserTextDecorations {
    pub background_color: Option<Vec4f>,
    pub decoration_color: Option<Vec4f>,
    pub underline: bool,
    pub overline: bool,
    pub line_through: bool,
    pub shadows: Vec<MpBrowserTextShadow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MpBrowserFontResource {
    pub key: u64,
    pub bytes: Arc<[u8]>,
    pub face_index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MpBrowserGlyphInstance {
    pub glyph_id: u32,
    pub font_size_px: f32,
    /// Primitive-local glyph origin / baseline anchor point.
    ///
    /// `x` is pen advance plus glyph `x_offset`.
    /// `y` is baseline ascent plus glyph `y_offset`.
    /// This stays primitive-local through the entire pipeline. Host, widget,
    /// and scene offsets are forbidden here.
    pub origin: DVec2,
    pub font_slot: u16,
}

/// Browser text run with split placement state.
///
/// `local_rect` stores the resolved text run bounds in origin space. Its
/// position is the run's top-left in origin space. For direct text this is
/// scene-space. For task-scene text this is task-local, typically `(0, 0)`.
///
/// `glyphs[].origin` stores each primitive-local glyph anchor point at the
/// baseline. It does not encode scene placement or host offsets.
///
/// At draw time the compositor computes the final glyph anchor as
/// `local_rect.pos + glyph.origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct MpBrowserTextRun {
    pub stable_id: MpBrowserStableTextRunId,
    pub local_rect: Rect,
    pub transform_id: MpPrimitiveTransformId,
    pub clip_chain_id: MpPrimitiveClipChainId,
    pub color: Vec4f,
    pub fonts: Vec<MpBrowserFontResource>,
    pub glyphs: Vec<MpBrowserGlyphInstance>,
    pub metrics: MpBrowserTextMetrics,
    pub decorations: MpBrowserTextDecorations,
}

#[derive(Clone, Debug)]
pub enum MpBrowserTaskKind {
    Scene(Box<MpBrowserScene>),
    Blur { input: MpBrowserTaskId, radius: f32 },
}

#[derive(Clone, Debug)]
pub struct MpBrowserTask {
    pub size: DVec2,
    pub cache_key: Option<MpBrowserCacheKey>,
    pub kind: MpBrowserTaskKind,
}

#[derive(Clone, Debug)]
pub struct MpBrowserPicture {
    pub local_rect: Rect,
    pub transform_id: MpPrimitiveTransformId,
    pub clip_chain_id: MpPrimitiveClipChainId,
    pub task_id: MpBrowserTaskId,
    pub opacity: f32,
    pub blend_mode: MpBlendMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpBrowserSceneItem {
    PrimitiveBatch(MpPrimitiveBatchId),
    TextRun(MpBrowserTextRunId),
    Picture(MpBrowserPictureId),
}

#[derive(Clone, Debug)]
pub struct MpBrowserScene {
    pub retained_scene_id: MpBrowserRetainedSceneId,
    pub host_rect: Rect,
    pub primitive_scene: MpBrowserPrimitiveScene,
    pub text_runs: Vec<MpBrowserTextRun>,
    pub tasks: Vec<MpBrowserTask>,
    pub pictures: Vec<MpBrowserPicture>,
    pub draw_order: Vec<MpBrowserSceneItem>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MpBrowserSceneFrameStats {
    pub offscreen_task_count: usize,
    pub scratch_surface_count: usize,
    pub scratch_surface_new_alloc_count: usize,
    pub scratch_surface_reuse_count: usize,
    pub task_cache_hit_count: usize,
    pub total_offscreen_pixel_area: u64,
    pub prepared_text_batch_hit_count: usize,
    pub prepared_text_batch_miss_count: usize,
    pub prepared_text_batch_rebuild_count: usize,
    pub glyph_residency_hit_count: usize,
    pub glyph_residency_miss_count: usize,
    pub glyph_cache_reset_count: usize,
    pub atlas_page_alloc_count: usize,
    pub msdf_request_queue_count: usize,
    pub msdf_completion_count: usize,
    pub synchronous_fallback_glyph_generation_count: usize,
}

impl MpBrowserScene {
    pub fn new(host_rect: Rect) -> Self {
        Self::new_with_retained_scene_id(0, host_rect)
    }

    pub fn new_with_retained_scene_id(
        retained_scene_id: MpBrowserRetainedSceneId,
        host_rect: Rect,
    ) -> Self {
        Self {
            retained_scene_id,
            host_rect,
            primitive_scene: MpBrowserPrimitiveScene::new(host_rect),
            text_runs: Vec::new(),
            tasks: Vec::new(),
            pictures: Vec::new(),
            draw_order: Vec::new(),
        }
    }

    pub fn push_transform(&mut self, transform: MpPrimitiveTransform) -> MpPrimitiveTransformId {
        self.primitive_scene.push_transform(transform)
    }

    pub fn push_clip_chain(&mut self, clip_chain: MpPrimitiveClipChain) -> MpPrimitiveClipChainId {
        self.primitive_scene.push_clip_chain(clip_chain)
    }

    pub fn push_primitive(&mut self, primitive: MpBrowserPrimitive) -> usize {
        let prev_batch_count = self.primitive_scene.batches.len();
        let primitive_id = self.primitive_scene.push_primitive(primitive);
        if self.primitive_scene.batches.len() > prev_batch_count {
            self.draw_order
                .push(MpBrowserSceneItem::PrimitiveBatch(prev_batch_count));
        }
        primitive_id
    }

    pub fn push_text_run(&mut self, text_run: MpBrowserTextRun) -> MpBrowserTextRunId {
        self.primitive_scene.break_batch();
        let id = self.text_runs.len();
        self.text_runs.push(text_run);
        self.draw_order.push(MpBrowserSceneItem::TextRun(id));
        id
    }

    pub fn push_task(&mut self, task: MpBrowserTask) -> MpBrowserTaskId {
        let id = self.tasks.len();
        self.tasks.push(task);
        id
    }

    pub fn push_picture(&mut self, picture: MpBrowserPicture) -> MpBrowserPictureId {
        self.primitive_scene.break_batch();
        let id = self.pictures.len();
        self.pictures.push(picture);
        self.draw_order.push(MpBrowserSceneItem::Picture(id));
        id
    }

    pub fn is_empty(&self) -> bool {
        self.draw_order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_primitives::MpBrowserPrimitiveKind;

    #[test]
    fn browser_scene_keeps_primitive_and_picture_order() {
        let mut scene = MpBrowserScene::new(Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(100.0, 100.0),
        });
        scene.push_primitive(MpBrowserPrimitive {
            local_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(10.0, 10.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            kind: MpBrowserPrimitiveKind::SolidRect {
                color: vec4(1.0, 0.0, 0.0, 1.0),
            },
        });
        let task_id = scene.push_task(MpBrowserTask {
            size: dvec2(10.0, 10.0),
            cache_key: None,
            kind: MpBrowserTaskKind::Scene(Box::new(MpBrowserScene::new(Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(10.0, 10.0),
            }))),
        });
        scene.push_picture(MpBrowserPicture {
            local_rect: Rect {
                pos: dvec2(10.0, 0.0),
                size: dvec2(10.0, 10.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            task_id,
            opacity: 1.0,
            blend_mode: MpBlendMode::Normal,
        });
        scene.push_primitive(MpBrowserPrimitive {
            local_rect: Rect {
                pos: dvec2(20.0, 0.0),
                size: dvec2(10.0, 10.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            kind: MpBrowserPrimitiveKind::SolidRect {
                color: vec4(0.0, 1.0, 0.0, 1.0),
            },
        });

        assert_eq!(
            scene.draw_order,
            vec![
                MpBrowserSceneItem::PrimitiveBatch(0),
                MpBrowserSceneItem::Picture(0),
                MpBrowserSceneItem::PrimitiveBatch(1),
            ]
        );
    }
}
