use makepad_widgets::DVec2;

use crate::{scene::{MpScene, MpSceneItem}, MpClipChainId, MpPrimitiveId, MpSpatialId};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct MpHitTestTag(pub u64);

#[derive(Clone, Debug)]
pub struct MpHitTestItem {
    pub tag: MpHitTestTag,
    pub primitive_id: MpPrimitiveId,
    pub spatial_id: MpSpatialId,
    pub clip_chain_id: MpClipChainId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpHitTestResult {
    pub tag: MpHitTestTag,
    pub primitive_id: MpPrimitiveId,
}

pub fn hit_test(scene: &MpScene, point: DVec2) -> Vec<MpHitTestResult> {
    let mut hits = Vec::new();
    for item in scene.items.iter().rev() {
        match item {
            MpSceneItem::Primitive(primitive_id) => {
                let primitive = &scene.primitives[primitive_id.0];
                let Some(tag) = primitive.hit_test_tag else {
                    continue;
                };
                let Ok(rect) = scene.resolve_primitive_rect(primitive) else {
                    continue;
                };
                if point.x >= rect.pos.x
                    && point.y >= rect.pos.y
                    && point.x <= rect.pos.x + rect.size.x
                    && point.y <= rect.pos.y + rect.size.y
                {
                    hits.push(MpHitTestResult {
                        tag,
                        primitive_id: primitive.id,
                    });
                }
            }
            MpSceneItem::Embed(embed_id) => {
                let embed = &scene.embeds[embed_id.0];
                let Some(tag) = embed.hit_test_tag else {
                    continue;
                };
                let Ok(rect) = scene.resolve_embed_rect(embed) else {
                    continue;
                };
                if point.x >= rect.pos.x
                    && point.y >= rect.pos.y
                    && point.x <= rect.pos.x + rect.size.x
                    && point.y <= rect.pos.y + rect.size.y
                {
                    hits.push(MpHitTestResult {
                        tag,
                        primitive_id: MpPrimitiveId::default(),
                    });
                }
            }
        }
    }
    hits
}
