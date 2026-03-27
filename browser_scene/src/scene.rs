use std::collections::HashMap;

use makepad_widgets::{dvec2, vec4, DVec2, Mat4f, Rect};

use crate::{
    clip::{MpClipChain, MpClipChainId, MpClipId, MpClipKind, MpClipNode},
    effect::{MpEffectId, MpEffectNode},
    embed::{MpEmbed, MpEmbedId, MpPipelineId},
    hit_test::MpHitTestItem,
    primitive::{MpPrimitive, MpPrimitiveId},
    resource::{MpGlyphRunKey, MpGlyphRunResource},
    spatial::{MpReferenceFrame, MpSpatialId, MpSpatialKind, MpSpatialNode},
    MpBackfaceVisibility, MpTransformStyle,
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpDocumentId(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpSceneId(pub u64);

#[derive(Clone, Debug)]
pub struct MpChildDocument {
    pub pipeline_id: MpPipelineId,
    pub document: Box<MpDocument>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MpSceneItem {
    Primitive(MpPrimitiveId),
    Embed(MpEmbedId),
}

#[derive(Clone, Debug)]
pub struct MpDocument {
    pub id: MpDocumentId,
    pub epoch: u64,
    pub scene: MpScene,
    pub glyph_runs: HashMap<MpGlyphRunKey, MpGlyphRunResource>,
    pub child_documents: Vec<MpChildDocument>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MpResolvedRoundedClip {
    pub rect: Rect,
    pub radius: crate::MpPerCornerRadius,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MpResolvedSpatialExecution {
    pub flat: Mat4f,
    pub projection: Mat4f,
    pub projected: Mat4f,
    pub backface_visibility: MpBackfaceVisibility,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MpResolvedClipState {
    pub clip_rect: Option<Rect>,
    pub rounded_clips: Vec<MpResolvedRoundedClip>,
}

#[derive(Clone, Debug)]
pub struct MpScene {
    pub id: MpSceneId,
    pub root_spatial_id: MpSpatialId,
    pub root_clip_chain_id: MpClipChainId,
    pub spatial_nodes: Vec<MpSpatialNode>,
    pub clips: Vec<MpClipNode>,
    pub clip_chains: Vec<MpClipChain>,
    pub effects: Vec<MpEffectNode>,
    pub primitives: Vec<MpPrimitive>,
    pub embeds: Vec<MpEmbed>,
    pub items: Vec<MpSceneItem>,
    pub hit_test_items: Vec<MpHitTestItem>,
}

impl MpDocument {
    pub fn new(id: MpDocumentId, scene: MpScene) -> Self {
        Self {
            id,
            epoch: 0,
            scene,
            glyph_runs: HashMap::new(),
            child_documents: Vec::new(),
        }
    }

    pub fn push_child_document(&mut self, pipeline_id: MpPipelineId, document: MpDocument) {
        self.child_documents.push(MpChildDocument {
            pipeline_id,
            document: Box::new(document),
        });
    }

    pub fn child_document(&self, pipeline_id: MpPipelineId) -> Option<&MpDocument> {
        self.child_documents
            .iter()
            .find(|child| child.pipeline_id == pipeline_id)
            .map(|child| child.document.as_ref())
    }
}

impl MpScene {
    pub fn new(id: MpSceneId, viewport_rect: Rect) -> Self {
        Self {
            id,
            root_spatial_id: MpSpatialId(0),
            root_clip_chain_id: MpClipChainId(0),
            spatial_nodes: vec![MpSpatialNode {
                parent: None,
                kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
                    viewport_rect,
                    placement_origin: viewport_rect.pos,
                    transform: None,
                    perspective: None,
                    transform_style: MpTransformStyle::Flat,
                    backface_visibility: MpBackfaceVisibility::Visible,
                    flattens_descendants: true,
                }),
            }],
            clips: Vec::new(),
            clip_chains: vec![MpClipChain::root()],
            effects: Vec::new(),
            primitives: Vec::new(),
            embeds: Vec::new(),
            items: Vec::new(),
            hit_test_items: Vec::new(),
        }
    }

    pub fn push_spatial_node(&mut self, node: MpSpatialNode) -> MpSpatialId {
        let id = MpSpatialId(self.spatial_nodes.len());
        self.spatial_nodes.push(node);
        id
    }

    pub fn push_clip(&mut self, clip: MpClipNode) -> MpClipId {
        let id = MpClipId(self.clips.len());
        self.clips.push(clip);
        id
    }

    pub fn push_clip_chain(&mut self, chain: MpClipChain) -> MpClipChainId {
        let id = MpClipChainId(self.clip_chains.len());
        self.clip_chains.push(chain);
        id
    }

    pub fn set_root_clip_chain(&mut self, clip_chain_id: MpClipChainId) {
        self.root_clip_chain_id = clip_chain_id;
    }

    pub fn push_effect(&mut self, effect: MpEffectNode) -> MpEffectId {
        let id = MpEffectId(self.effects.len());
        self.effects.push(effect);
        id
    }

    pub fn push_primitive(&mut self, mut primitive: MpPrimitive) -> MpPrimitiveId {
        let id = MpPrimitiveId(self.primitives.len());
        primitive.id = id;
        self.primitives.push(primitive);
        self.items.push(MpSceneItem::Primitive(id));
        id
    }

    pub fn push_embed(&mut self, embed: MpEmbed) -> MpEmbedId {
        let id = MpEmbedId(self.embeds.len());
        self.embeds.push(embed);
        self.items.push(MpSceneItem::Embed(id));
        id
    }

    pub fn push_hit_test_item(&mut self, item: MpHitTestItem) {
        self.hit_test_items.push(item);
    }

    pub fn set_scroll_offset(&mut self, spatial_id: MpSpatialId, scroll_offset: DVec2) -> bool {
        let Some(node) = self.spatial_nodes.get_mut(spatial_id.0) else {
            return false;
        };
        let MpSpatialKind::ScrollFrame(frame) = &mut node.kind else {
            return false;
        };
        frame.scroll_offset = scroll_offset;
        true
    }

    pub fn update_scroll_offsets(
        &mut self,
        offsets: impl IntoIterator<Item = (MpSpatialId, DVec2)>,
    ) -> usize {
        let mut updated = 0;
        for (spatial_id, scroll_offset) in offsets {
            updated += self.set_scroll_offset(spatial_id, scroll_offset) as usize;
        }
        updated
    }

    pub fn root_viewport_rect(&self) -> Rect {
        match &self.spatial_nodes[self.root_spatial_id.0].kind {
            MpSpatialKind::ReferenceFrame(frame) => frame.viewport_rect,
            _ => Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(0.0, 0.0),
            },
        }
    }

    pub fn resolve_primitive_rect(&self, primitive: &MpPrimitive) -> Result<Rect, &'static str> {
        self.resolve_rect_in_origin_space(primitive.spatial_id, primitive.bounds, None)
    }

    pub fn resolve_embed_rect(&self, embed: &MpEmbed) -> Result<Rect, &'static str> {
        self.resolve_rect_in_origin_space(embed.spatial_id, embed.bounds, None)
    }

    pub fn find_transform_ancestor(&self, spatial_id: MpSpatialId) -> Option<MpSpatialId> {
        let mut current = Some(spatial_id);
        while let Some(id) = current {
            let node = self.spatial_nodes.get(id.0)?;
            if matches!(
                &node.kind,
                MpSpatialKind::ReferenceFrame(frame)
                    if frame.perspective.is_some()
                        || matches!(frame.transform, Some(transform) if !is_translation_only_transform(transform))
                        || matches!(frame.transform_style, MpTransformStyle::Preserve3D)
            ) {
                return Some(id);
            }
            current = node.parent;
        }
        None
    }

    pub fn resolve_spatial_transform(&self, spatial_id: MpSpatialId) -> Mat4f {
        let lineage = self.spatial_lineage(spatial_id);
        let mut transform = Mat4f::identity();
        let mut nearest_scroll_port_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: self.root_viewport_rect().size,
        };
        for (_, node) in lineage {
            transform = Mat4f::mul(
                &transform,
                &spatial_local_transform(node, &mut nearest_scroll_port_rect),
            );
        }
        transform
    }

    pub(crate) fn resolve_spatial_execution(
        &self,
        spatial_id: MpSpatialId,
    ) -> Result<MpResolvedSpatialExecution, &'static str> {
        let lineage = self.spatial_lineage_checked(spatial_id)?;
        let mut flat = Mat4f::identity();
        let mut descendant_projection = Mat4f::identity();
        let mut projection = Mat4f::identity();
        let mut nearest_scroll_port_rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: self.root_viewport_rect().size,
        };
        let mut backface_visibility = MpBackfaceVisibility::Visible;
        for (id, node) in lineage {
            flat = Mat4f::mul(
                &flat,
                &spatial_local_transform(node, &mut nearest_scroll_port_rect),
            );
            if let MpSpatialKind::ReferenceFrame(frame) = &node.kind {
                projection = descendant_projection;
                if let Some(perspective) = frame.perspective {
                    projection = Mat4f::mul(&perspective, &projection);
                }
                backface_visibility = frame.backface_visibility;
                if id == spatial_id {
                    break;
                }
                descendant_projection = if frame.flattens_descendants {
                    Mat4f::identity()
                } else {
                    projection
                };
            }
        }
        Ok(MpResolvedSpatialExecution {
            flat,
            projection,
            projected: Mat4f::mul(&projection, &flat),
            backface_visibility,
        })
    }

    pub(crate) fn resolve_spatial_flat_transform_from(
        &self,
        spatial_id: MpSpatialId,
        origin_spatial_id: MpSpatialId,
    ) -> Result<Mat4f, &'static str> {
        let execution = self.resolve_spatial_execution(spatial_id)?;
        let origin_execution = self.resolve_spatial_execution(origin_spatial_id)?;
        Ok(Mat4f::mul(&origin_execution.flat.invert(), &execution.flat))
    }

    pub(crate) fn resolve_rect_transform(
        &self,
        spatial_id: MpSpatialId,
        origin_spatial_id: Option<MpSpatialId>,
    ) -> Result<Mat4f, &'static str> {
        match origin_spatial_id {
            Some(origin_spatial_id) => self.resolve_spatial_flat_transform_from(spatial_id, origin_spatial_id),
            None => Ok(self.resolve_spatial_execution(spatial_id)?.projected),
        }
    }

    pub(crate) fn resolve_rect_in_origin_space(
        &self,
        spatial_id: MpSpatialId,
        rect: Rect,
        origin_spatial_id: Option<MpSpatialId>,
    ) -> Result<Rect, &'static str> {
        let transform = self.resolve_rect_transform(spatial_id, origin_spatial_id)?;
        transform_rect(rect, transform)
    }

    pub(crate) fn resolve_clip_state(
        &self,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
    ) -> Result<MpResolvedClipState, &'static str> {
        self.resolve_clip_state_from(spatial_id, clip_chain_id, None)
    }

    pub(crate) fn resolve_clip_state_from(
        &self,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
        origin_spatial_id: Option<MpSpatialId>,
    ) -> Result<MpResolvedClipState, &'static str> {
        self.resolve_clip_state_from_internal(spatial_id, clip_chain_id, origin_spatial_id)
    }

    fn resolve_clip_state_from_internal(
        &self,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
        origin_spatial_id: Option<MpSpatialId>,
    ) -> Result<MpResolvedClipState, &'static str> {
        let item_rect_transform = self.resolve_rect_transform(spatial_id, origin_spatial_id)?;
        let mut current = Some(clip_chain_id);
        let mut resolved = MpResolvedClipState::default();
        while let Some(id) = current {
            let chain = self.clip_chains.get(id.0).ok_or("missing clip chain")?;
            for clip_id in &chain.clips {
                let clip = self.clips.get(clip_id.0).ok_or("missing clip")?;
                let clip_rect_transform = self.resolve_rect_transform(clip.spatial_id, origin_spatial_id)?;
                let clip_to_item = Mat4f::mul(&item_rect_transform.invert(), &clip_rect_transform);
                match &clip.kind {
                    MpClipKind::Rect { rect } => {
                        let rect = transform_rect(*rect, clip_to_item)?;
                        resolved.clip_rect = Some(intersect_rects(resolved.clip_rect, rect));
                    }
                    MpClipKind::RoundedRect { rect, radius } => {
                        if !transform_preserves_axis_alignment(clip_to_item)? {
                            return Err("unsupported transformed rounded clip state");
                        }
                        let rect = transform_rect(*rect, clip_to_item)?;
                        resolved.clip_rect = Some(intersect_rects(resolved.clip_rect, rect));
                        resolved.rounded_clips.push(MpResolvedRoundedClip {
                            rect,
                            radius: *radius,
                        });
                    }
                    MpClipKind::ImageMask { .. } | MpClipKind::PlaneSet { .. } => {
                        return Err("unsupported clip kind in initial renderer")
                    }
                }
            }
            current = chain.parent;
        }
        Ok(resolved)
    }

    pub fn resolve_clip_rect(
        &self,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
    ) -> Result<Option<Rect>, &'static str> {
        Ok(self.resolve_clip_state(spatial_id, clip_chain_id)?.clip_rect)
    }

    pub(crate) fn clip_chain_clip_ids_root_to_leaf(
        &self,
        clip_chain_id: MpClipChainId,
    ) -> Result<Vec<MpClipId>, &'static str> {
        let mut chain_ids = Vec::new();
        let mut current = Some(clip_chain_id);
        while let Some(id) = current {
            let chain = self.clip_chains.get(id.0).ok_or("missing clip chain")?;
            chain_ids.push(id);
            current = chain.parent;
        }
        chain_ids.reverse();
        let mut clip_ids = Vec::new();
        for chain_id in chain_ids {
            let chain = self.clip_chains.get(chain_id.0).ok_or("missing clip chain")?;
            clip_ids.extend(chain.clips.iter().copied());
        }
        Ok(clip_ids)
    }

    pub fn resolve_spatial_translation(&self, spatial_id: MpSpatialId) -> Result<DVec2, &'static str> {
        transform_point_2d(self.resolve_spatial_execution(spatial_id)?.projected, dvec2(0.0, 0.0))
    }

    pub fn resolve_spatial_translation_from(
        &self,
        spatial_id: MpSpatialId,
        origin_id: MpSpatialId,
    ) -> Result<DVec2, &'static str> {
        transform_point_2d(
            self.resolve_spatial_flat_transform_from(spatial_id, origin_id)?,
            dvec2(0.0, 0.0),
        )
    }

    fn spatial_lineage(&self, spatial_id: MpSpatialId) -> Vec<(MpSpatialId, &MpSpatialNode)> {
        self.spatial_lineage_checked(spatial_id)
            .expect("valid spatial lineage")
    }

    fn spatial_lineage_checked(
        &self,
        spatial_id: MpSpatialId,
    ) -> Result<Vec<(MpSpatialId, &MpSpatialNode)>, &'static str> {
        let mut lineage = Vec::new();
        let mut current = Some(spatial_id);
        while let Some(id) = current {
            let node = self.spatial_nodes.get(id.0).ok_or("missing spatial node")?;
            lineage.push((id, node));
            current = node.parent;
        }
        lineage.reverse();
        Ok(lineage)
    }
}

fn transform_point_2d(transform: Mat4f, point: DVec2) -> Result<DVec2, &'static str> {
    let point = transform.transform_vec4(vec4(point.x as f32, point.y as f32, 0.0, 1.0));
    if point.w.abs() <= 1e-6 {
        return Err("degenerate projected point");
    }
    Ok(dvec2(
        (point.x / point.w) as f64,
        (point.y / point.w) as f64,
    ))
}

fn transform_rect(rect: Rect, transform: Mat4f) -> Result<Rect, &'static str> {
    let corners = [
        transform_point_2d(transform, rect.pos)?,
        transform_point_2d(transform, dvec2(rect.pos.x + rect.size.x, rect.pos.y))?,
        transform_point_2d(transform, dvec2(rect.pos.x, rect.pos.y + rect.size.y))?,
        transform_point_2d(
            transform,
            dvec2(rect.pos.x + rect.size.x, rect.pos.y + rect.size.y),
        )?,
    ];
    let min_x = corners.iter().fold(f64::INFINITY, |min, point| min.min(point.x));
    let min_y = corners.iter().fold(f64::INFINITY, |min, point| min.min(point.y));
    let max_x = corners.iter().fold(f64::NEG_INFINITY, |max, point| max.max(point.x));
    let max_y = corners.iter().fold(f64::NEG_INFINITY, |max, point| max.max(point.y));
    Ok(Rect {
        pos: dvec2(min_x, min_y),
        size: dvec2(max_x - min_x, max_y - min_y),
    })
}

pub(crate) fn transform_preserves_axis_alignment(transform: Mat4f) -> Result<bool, &'static str> {
    let corners = [
        transform_point_2d(transform, dvec2(0.0, 0.0))?,
        transform_point_2d(transform, dvec2(1.0, 0.0))?,
        transform_point_2d(transform, dvec2(0.0, 1.0))?,
        transform_point_2d(transform, dvec2(1.0, 1.0))?,
    ];
    let epsilon = 1e-6;
    let top_horizontal = (corners[0].y - corners[1].y).abs() <= epsilon;
    let top_vertical = (corners[0].x - corners[1].x).abs() <= epsilon;
    let left_horizontal = (corners[0].y - corners[2].y).abs() <= epsilon;
    let left_vertical = (corners[0].x - corners[2].x).abs() <= epsilon;
    Ok((top_horizontal || top_vertical)
        && (left_horizontal || left_vertical)
        && top_horizontal != left_horizontal)
}

fn is_translation_only_transform(transform: Mat4f) -> bool {
    transform.v[0] == 1.0
        && transform.v[1] == 0.0
        && transform.v[2] == 0.0
        && transform.v[3] == 0.0
        && transform.v[4] == 0.0
        && transform.v[5] == 1.0
        && transform.v[6] == 0.0
        && transform.v[7] == 0.0
        && transform.v[8] == 0.0
        && transform.v[9] == 0.0
        && transform.v[10] == 1.0
        && transform.v[11] == 0.0
        && transform.v[14] == 0.0
        && transform.v[15] == 1.0
}

fn spatial_local_transform(
    node: &MpSpatialNode,
    nearest_scroll_port_rect: &mut Rect,
) -> Mat4f {
    match &node.kind {
        MpSpatialKind::ReferenceFrame(frame) => {
            let mut transform = translation_matrix(frame.placement_origin);
            if let Some(local_transform) = frame.transform {
                transform = Mat4f::mul(&transform, &local_transform);
            }
            transform
        }
        MpSpatialKind::ScrollFrame(frame) => {
            *nearest_scroll_port_rect = Rect {
                pos: frame.scroll_offset,
                size: frame.viewport_rect.size,
            };
            translation_matrix(frame.viewport_rect.pos - frame.scroll_offset)
        }
        MpSpatialKind::StickyFrame(frame) => {
            translation_matrix(frame.frame_rect.pos + sticky_offset(frame, *nearest_scroll_port_rect))
        }
        MpSpatialKind::EmbedRoot(root) => translation_matrix(root.rect.pos),
    }
}

fn translation_matrix(offset: DVec2) -> Mat4f {
    Mat4f {
        v: [
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            offset.x as f32,
            offset.y as f32,
            0.0,
            1.0,
        ],
    }
}

fn sticky_offset(frame: &crate::spatial::MpStickyFrame, scroll_port_rect: Rect) -> DVec2 {
    if frame.margins.top.is_none()
        && frame.margins.right.is_none()
        && frame.margins.bottom.is_none()
        && frame.margins.left.is_none()
    {
        return dvec2(0.0, 0.0);
    }

    let mut sticky_rect = frame.frame_rect;
    let mut offset = dvec2(0.0, 0.0);

    if let Some(margin) = frame.margins.top {
        let top_viewport_edge = scroll_port_rect.pos.y + margin as f64;
        if sticky_rect.pos.y < top_viewport_edge {
            offset.y = top_viewport_edge - sticky_rect.pos.y;
        }
    }

    if offset.y <= 0.0 {
        if let Some(margin) = frame.margins.bottom {
            sticky_rect.pos.y += offset.y;
            let bottom_viewport_edge =
                scroll_port_rect.pos.y + scroll_port_rect.size.y - margin as f64;
            let sticky_bottom = sticky_rect.pos.y + sticky_rect.size.y;
            if sticky_bottom > bottom_viewport_edge {
                offset.y += bottom_viewport_edge - sticky_bottom;
            }
        }
    }

    if let Some(margin) = frame.margins.left {
        let left_viewport_edge = scroll_port_rect.pos.x + margin as f64;
        if sticky_rect.pos.x < left_viewport_edge {
            offset.x = left_viewport_edge - sticky_rect.pos.x;
        }
    }

    if offset.x <= 0.0 {
        if let Some(margin) = frame.margins.right {
            sticky_rect.pos.x += offset.x;
            let right_viewport_edge =
                scroll_port_rect.pos.x + scroll_port_rect.size.x - margin as f64;
            let sticky_right = sticky_rect.pos.x + sticky_rect.size.x;
            if sticky_right > right_viewport_edge {
                offset.x += right_viewport_edge - sticky_right;
            }
        }
    }

    let frame_left = frame.frame_rect.pos.x;
    let frame_top = frame.frame_rect.pos.y;
    let frame_right = frame.frame_rect.pos.x + frame.frame_rect.size.x;
    let frame_bottom = frame.frame_rect.pos.y + frame.frame_rect.size.y;
    let cb_left = frame.containing_block_rect.pos.x;
    let cb_top = frame.containing_block_rect.pos.y;
    let cb_right = frame.containing_block_rect.pos.x + frame.containing_block_rect.size.x;
    let cb_bottom = frame.containing_block_rect.pos.y + frame.containing_block_rect.size.y;
    offset.x = offset.x.max(cb_left - frame_left).min(cb_right - frame_right);
    offset.y = offset.y.max(cb_top - frame_top).min(cb_bottom - frame_bottom);

    offset
}

fn intersect_rects(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => {
            let min_x = a.pos.x.max(b.pos.x);
            let min_y = a.pos.y.max(b.pos.y);
            let max_x = (a.pos.x + a.size.x).min(b.pos.x + b.size.x);
            let max_y = (a.pos.y + a.size.y).min(b.pos.y + b.size.y);
            Rect {
                pos: dvec2(min_x, min_y),
                size: dvec2((max_x - min_x).max(0.0), (max_y - min_y).max(0.0)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{primitive::MpPrimitive, MpClipChain, MpClipKind, MpClipNode};

    #[test]
    fn resolves_clip_chain_intersection() {
        let mut scene = MpScene::new(
            MpSceneId(7),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let outer = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::Rect {
                rect: Rect {
                    pos: dvec2(10.0, 10.0),
                    size: dvec2(100.0, 100.0),
                },
            },
        });
        let outer_chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![outer],
        });
        let inner = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::Rect {
                rect: Rect {
                    pos: dvec2(50.0, 40.0),
                    size: dvec2(80.0, 90.0),
                },
            },
        });
        let inner_chain = scene.push_clip_chain(MpClipChain {
            parent: Some(outer_chain),
            clips: vec![inner],
        });

        let resolved = scene
            .resolve_clip_rect(scene.root_spatial_id, inner_chain)
            .unwrap()
            .unwrap();

        assert_eq!(resolved.pos, dvec2(50.0, 40.0));
        assert_eq!(resolved.size, dvec2(60.0, 70.0));
    }

    #[test]
    fn can_replace_root_clip_chain() {
        let mut scene = MpScene::new(
            MpSceneId(9),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let clip = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::Rect {
                rect: Rect {
                    pos: dvec2(20.0, 20.0),
                    size: dvec2(80.0, 60.0),
                },
            },
        });
        let chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![clip],
        });

        scene.set_root_clip_chain(chain);

        assert_eq!(scene.root_clip_chain_id, chain);
        assert_eq!(
            scene.resolve_clip_rect(scene.root_spatial_id, scene.root_clip_chain_id)
                .unwrap()
                .unwrap(),
            Rect {
                pos: dvec2(20.0, 20.0),
                size: dvec2(80.0, 60.0),
            }
        );
    }

    #[test]
    fn updates_scroll_frame_offsets_in_place() {
        let mut scene = MpScene::new(
            MpSceneId(11),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let scroll_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ScrollFrame(crate::MpScrollFrame {
                viewport_rect: Rect {
                    pos: dvec2(10.0, 20.0),
                    size: dvec2(200.0, 100.0),
                },
                content_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 500.0),
                },
                scroll_offset: dvec2(0.0, 0.0),
            }),
        });

        assert!(scene.set_scroll_offset(scroll_id, dvec2(5.0, 40.0)));
        assert_eq!(
            scene.resolve_spatial_translation(scroll_id).unwrap(),
            dvec2(10.0 - 5.0, 20.0 - 40.0)
        );
        assert_eq!(
            scene.update_scroll_offsets([
                (scroll_id, dvec2(7.0, 55.0)),
                (scene.root_spatial_id, dvec2(1.0, 1.0)),
            ]),
            1
        );
        assert_eq!(
            scene.resolve_spatial_translation(scroll_id).unwrap(),
            dvec2(10.0 - 7.0, 20.0 - 55.0)
        );
    }

    #[test]
    fn resolves_exact_rounded_clip_state() {
        let mut scene = MpScene::new(
            MpSceneId(8),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let outer = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::Rect {
                rect: Rect {
                    pos: dvec2(10.0, 10.0),
                    size: dvec2(200.0, 160.0),
                },
            },
        });
        let outer_chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![outer],
        });
        let rounded = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::RoundedRect {
                rect: Rect {
                    pos: dvec2(40.0, 30.0),
                    size: dvec2(100.0, 90.0),
                },
                radius: crate::MpPerCornerRadius::uniform(12.0),
            },
        });
        let chain = scene.push_clip_chain(MpClipChain {
            parent: Some(outer_chain),
            clips: vec![rounded],
        });

        let resolved = scene
            .resolve_clip_state(scene.root_spatial_id, chain)
            .unwrap();
        let clip_rect = resolved.clip_rect.unwrap();

        assert_eq!(clip_rect.pos, dvec2(40.0, 30.0));
        assert_eq!(clip_rect.size, dvec2(100.0, 90.0));
        assert_eq!(resolved.rounded_clips.len(), 1);
        assert_eq!(resolved.rounded_clips[0].rect.pos, dvec2(40.0, 30.0));
        assert_eq!(resolved.rounded_clips[0].rect.size, dvec2(100.0, 90.0));
        assert_eq!(
            resolved.rounded_clips[0].radius,
            crate::MpPerCornerRadius::uniform(12.0)
        );
    }

    #[test]
    fn resolves_sticky_translation_inside_scroll_frame() {
        let mut scene = MpScene::new(
            MpSceneId(10),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let scroll_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ScrollFrame(crate::MpScrollFrame {
                viewport_rect: Rect {
                    pos: dvec2(20.0, 30.0),
                    size: dvec2(200.0, 100.0),
                },
                content_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 500.0),
                },
                scroll_offset: dvec2(0.0, 120.0),
            }),
        });
        let sticky_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scroll_id),
            kind: crate::MpSpatialKind::StickyFrame(crate::MpStickyFrame {
                frame_rect: Rect {
                    pos: dvec2(0.0, 80.0),
                    size: dvec2(100.0, 20.0),
                },
                containing_block_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 500.0),
                },
                margins: crate::MpStickyOffsets {
                    top: Some(10.0),
                    right: None,
                    bottom: None,
                    left: None,
                },
            }),
        });

        let translation = scene.resolve_spatial_translation(sticky_id).unwrap();

        assert_eq!(translation, dvec2(20.0, 40.0));
    }

    #[test]
    fn resolves_primitive_rect_through_scaled_reference_frame() {
        let mut scene = MpScene::new(
            MpSceneId(15),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let scale_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 120.0),
                },
                placement_origin: dvec2(20.0, 30.0),
                transform: Some(Mat4f {
                    v: [
                        2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        let primitive = MpPrimitive::solid_rect(
            crate::MpPrimitiveId(0),
            scale_id,
            scene.root_clip_chain_id,
            Rect {
                pos: dvec2(5.0, 7.0),
                size: dvec2(10.0, 12.0),
            },
            vec4(1.0, 0.0, 0.0, 1.0),
        );

        assert_eq!(
            scene.resolve_primitive_rect(&primitive).unwrap(),
            Rect {
                pos: dvec2(30.0, 44.0),
                size: dvec2(20.0, 24.0),
            }
        );
    }

    #[test]
    fn finds_innermost_transform_ancestor() {
        let mut scene = MpScene::new(
            MpSceneId(12),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let outer = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(100.0, 100.0),
                },
                placement_origin: dvec2(20.0, 30.0),
                transform: Some(Mat4f {
                    v: [
                        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        let inner = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(outer),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(80.0, 60.0),
                },
                placement_origin: dvec2(5.0, 7.0),
                transform: Some(Mat4f {
                    v: [
                        2.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        let leaf = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(inner),
            kind: crate::MpSpatialKind::ScrollFrame(crate::MpScrollFrame {
                viewport_rect: Rect {
                    pos: dvec2(3.0, 4.0),
                    size: dvec2(40.0, 30.0),
                },
                content_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(40.0, 60.0),
                },
                scroll_offset: dvec2(1.0, 2.0),
            }),
        });

        assert_eq!(scene.find_transform_ancestor(leaf), Some(inner));
    }

    #[test]
    fn resolves_clip_through_transform_ancestor() {
        // Clip is on parent, transform ancestor (scale) is child of parent,
        // item is child of transform ancestor.  resolve_clip_state_from must
        // resolve the clip's spatial translation relative to the transform
        // ancestor even though the clip is above the non-translation transform.
        let mut scene = MpScene::new(
            MpSceneId(14),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(800.0, 600.0),
            },
        );
        // Parent spatial: translation-only, hosts the overflow:hidden clip.
        let parent_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(800.0, 600.0),
                },
                placement_origin: dvec2(0.0, 0.0),
                transform: None,
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        // Transform ancestor: scale(1.02), non-identity transform.
        let transform_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(parent_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(400.0, 300.0),
                },
                placement_origin: dvec2(200.0, 150.0),
                transform: Some(Mat4f {
                    v: [
                        1.02, 0.0, 0.0, 0.0,
                        0.0, 1.02, 0.0, 0.0,
                        0.0, 0.0, 1.0, 0.0,
                        0.0, 0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        // Item spatial: child of transform ancestor.
        let item_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(transform_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(200.0, 100.0),
                },
                placement_origin: dvec2(50.0, 50.0),
                transform: None,
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        // Clip on parent (overflow:hidden).
        let clip = scene.push_clip(MpClipNode {
            spatial_id: parent_id,
            kind: MpClipKind::Rect {
                rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(800.0, 600.0),
                },
            },
        });
        let clip_chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![clip],
        });

        // resolve_clip_state_from with origin = transform_id must succeed.
        let result = scene.resolve_clip_state_from(item_id, clip_chain, Some(transform_id));
        assert!(result.is_ok(), "clip resolution through transform ancestor failed: {:?}", result.err());

        // Also verify resolve_spatial_translation_from now uses the full
        // relative transform, not the old placement-only subtraction.
        let clip_offset = scene.resolve_spatial_translation_from(parent_id, transform_id);
        assert!(clip_offset.is_ok(), "spatial translation from clip to transform ancestor failed");
        let clip_offset = clip_offset.unwrap();
        assert!((clip_offset.x + 196.0784454345703).abs() < 1e-6);
        assert!((clip_offset.y + 147.058837890625).abs() < 1e-6);
    }

    #[test]
    fn resolves_translation_from_transform_ancestor() {
        let mut scene = MpScene::new(
            MpSceneId(13),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let transform_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(scene.root_spatial_id),
            kind: crate::MpSpatialKind::ReferenceFrame(crate::MpReferenceFrame {
                viewport_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(100.0, 100.0),
                },
                placement_origin: dvec2(20.0, 30.0),
                transform: Some(Mat4f {
                    v: [
                        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
                        0.0, 0.0, 1.0,
                    ],
                }),
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        });
        let scroll_id = scene.push_spatial_node(crate::MpSpatialNode {
            parent: Some(transform_id),
            kind: crate::MpSpatialKind::ScrollFrame(crate::MpScrollFrame {
                viewport_rect: Rect {
                    pos: dvec2(10.0, 12.0),
                    size: dvec2(40.0, 30.0),
                },
                content_rect: Rect {
                    pos: dvec2(0.0, 0.0),
                    size: dvec2(40.0, 60.0),
                },
                scroll_offset: dvec2(3.0, 5.0),
            }),
        });

        assert_eq!(
            scene
                .resolve_spatial_translation_from(scroll_id, transform_id)
                .unwrap(),
            dvec2(7.0, 7.0)
        );
    }
}

