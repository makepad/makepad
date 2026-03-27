use makepad_widgets::{vec3, Mat4f, Rect};

use makepad_compositor::{
    MpBackfaceVisibility, MpBrowserScene as MpCompositorBrowserScene, MpPrimitiveClipChain,
    MpPrimitiveTransform,
};

use crate::scene::MpScene;

use super::{retained_patch, MpRenderError, MpSceneLowerer};

pub(super) fn lower_direct_transform(
    scene: &MpScene,
    host_rect: Rect,
    transform_spatial_id: Option<crate::MpSpatialId>,
) -> Result<MpPrimitiveTransform, MpRenderError> {
    let host_translation = Mat4f::translation(vec3(
        host_rect.pos.x as f32,
        host_rect.pos.y as f32,
        0.0,
    ));
    let Some(spatial_id) = transform_spatial_id else {
        return Ok(MpPrimitiveTransform {
            scene_from_origin: host_translation,
            backface_visibility: MpBackfaceVisibility::Visible,
        });
    };
    let execution = scene
        .resolve_spatial_execution(spatial_id)
        .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))?;
    Ok(MpPrimitiveTransform {
        scene_from_origin: Mat4f::mul(&host_translation, &execution.projected),
        backface_visibility: execution.backface_visibility,
    })
}

impl MpSceneLowerer<'_> {
    pub(super) fn ensure_transform(
        &mut self,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        scene: &MpScene,
        transform_spatial_id: Option<crate::MpSpatialId>,
    ) -> Result<usize, MpRenderError> {
        let source = retained_patch::RetainedTransformSource { transform_spatial_id };
        if let Some(transform_id) = patch_builder.transform_id_for_source(source) {
            return Ok(transform_id);
        }
        let transform = lower_direct_transform(scene, lowered.host_rect, transform_spatial_id)?;
        let transform_id = lowered.push_transform(transform);
        patch_builder.record_transform(transform_id, source);
        Ok(transform_id)
    }

    pub(super) fn ensure_clip_chain(
        &mut self,
        lowered: &mut MpCompositorBrowserScene,
        patch_builder: &mut retained_patch::ScenePatchBuilder,
        clip_chain: MpPrimitiveClipChain,
        source: retained_patch::RetainedClipChainSource,
    ) -> usize {
        if let Some(clip_chain_id) = patch_builder.clip_chain_id_for_source(source) {
            return clip_chain_id;
        }
        let clip_chain_id = lowered.push_clip_chain(clip_chain);
        patch_builder.record_clip_chain(clip_chain_id, source);
        clip_chain_id
    }
}
