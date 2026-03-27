pub use makepad_draw;
pub use makepad_draw::*;

pub mod browser_primitives;
pub mod browser_scene;
mod eval;
mod quad;
pub mod scene;
pub mod surface;

pub use crate::browser_primitives::{
    MpBrowserGradientStop, MpBrowserPrimitive, MpBrowserPrimitiveClass, MpBrowserPrimitiveKind,
    MpBrowserPrimitiveScene, MpPrimitiveBatch, MpPrimitiveBatchId, MpPrimitiveClipChain,
    MpPrimitiveClipChainId, MpPrimitiveClipEntry, MpPrimitiveTransform, MpPrimitiveTransformId,
};
pub use crate::browser_scene::{
    MpBrowserCacheKey, MpBrowserFontResource, MpBrowserGlyphInstance, MpBrowserPicture,
    MpBrowserPictureId, MpBrowserRetainedSceneId, MpBrowserScene, MpBrowserSceneFrameStats,
    MpBrowserSceneItem, MpBrowserStableTextRunId, MpBrowserTask, MpBrowserTaskId,
    MpBrowserTaskKind, MpBrowserTextDecorations, MpBrowserTextMetrics, MpBrowserTextRun,
    MpBrowserTextRunId, MpBrowserTextShadow, MpPreparedBrowserScene,
    MpPreparedBrowserTextRunKey,
};
pub use crate::scene::{
    MpBackfaceVisibility, MpBlendMode, MpClipNode, MpClipShape, MpEffectNode, MpEmbedNode,
    MpFilterSet, MpHit, MpHitTestOptions, MpMaskSource, MpNode, MpNodeId, MpProjectError,
    MpProjectedPoint, MpReferenceFrame, MpScene, MpSceneRoot, MpSurfaceNode, MpSurfaceSource,
    MpTransformStyle,
};
pub use crate::surface::{MpSurface, MpSurfaceColorFormat};

pub const MP_MAX_CLIP_PLANES: usize = crate::quad::MP_MAX_CLIP_PLANES;

pub struct MpRenderer {
    compositor: quad::MpCompositor,
}

impl MpRenderer {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            compositor: quad::MpCompositor::new(cx),
        }
    }

    pub fn draw_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpScene,
    ) -> Result<(), MpProjectError> {
        self.compositor.draw_scene(cx, scene)
    }

    pub fn draw_browser_primitive_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserPrimitiveScene,
    ) {
        self.compositor.draw_browser_primitive_scene(cx, scene)
    }

    pub fn draw_browser_scene(&mut self, cx: &mut Cx2d, scene: &MpBrowserScene) {
        self.compositor.draw_browser_scene(cx, scene)
    }

    pub fn draw_retained_browser_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &mut MpPreparedBrowserScene,
    ) {
        self.compositor
            .draw_retained_browser_scene(cx, scene, prepared_text)
    }

    pub fn browser_scene_frame_stats(&self) -> MpBrowserSceneFrameStats {
        self.compositor.browser_scene_frame_stats()
    }
}

pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    crate::browser_primitives::script_mod(vm);
    crate::browser_scene::script_mod(vm);
    crate::quad::script_mod(vm);
    NIL
}
