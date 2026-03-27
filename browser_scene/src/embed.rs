use makepad_widgets::Rect;

use crate::{MpClipChainId, MpEffectId, MpHitTestTag, MpSceneId, MpSpatialId};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpEmbedId(pub usize);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpPipelineId(pub u64);

#[derive(Clone, Debug)]
pub struct MpEmbed {
    pub scene_id: MpSceneId,
    pub pipeline_id: MpPipelineId,
    pub spatial_id: MpSpatialId,
    pub clip_chain_id: MpClipChainId,
    pub effect_id: Option<MpEffectId>,
    pub bounds: Rect,
    pub hit_test_tag: Option<MpHitTestTag>,
}
