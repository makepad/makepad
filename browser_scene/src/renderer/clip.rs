use makepad_widgets::{vec4, Mat4f, Rect, Vec4f};

use makepad_compositor::{MpPrimitiveClipChain, MpPrimitiveClipEntry};

use crate::{scene::{transform_preserves_axis_alignment, MpScene}, MpClipKind};

use super::{geom::{intersect_rects, radius_to_vec4}, MpRenderError};

pub(super) fn lower_clip_chain(
    scene: &MpScene,
    _spatial_id: crate::MpSpatialId,
    clip_chain_id: crate::MpClipChainId,
    origin_spatial_id: Option<crate::MpSpatialId>,
) -> Result<MpPrimitiveClipChain, MpRenderError> {
    let mut clip_chain = MpPrimitiveClipChain::default();
    let clip_ids = scene
        .clip_chain_clip_ids_root_to_leaf(clip_chain_id)
        .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))?;
    for clip_id in clip_ids {
        let clip = &scene.clips[clip_id.0];
        let clip_rect_transform = scene
            .resolve_rect_transform(clip.spatial_id, origin_spatial_id)
            .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))?;
        let mask_local_from_origin = clip_rect_transform.invert();
        let local_fast_path = transform_preserves_axis_alignment(clip_rect_transform)
            .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))?;
        match &clip.kind {
            MpClipKind::Rect { rect } => {
                if local_fast_path {
                    clip_chain.origin_clip_rect = Some(intersect_rects(
                        clip_chain.origin_clip_rect,
                        clip_rect_in_origin_space(scene, clip.spatial_id, *rect, origin_spatial_id)?,
                    ));
                } else {
                    clip_chain.entries.push(MpPrimitiveClipEntry::PlaneSet {
                        planes: transform_planes_to_origin(
                            &rect_as_plane_set(*rect),
                            clip_rect_transform,
                        ),
                    });
                }
            }
            MpClipKind::RoundedRect { rect, radius } => {
                if local_fast_path {
                    clip_chain.origin_clip_rect = Some(intersect_rects(
                        clip_chain.origin_clip_rect,
                        clip_rect_in_origin_space(scene, clip.spatial_id, *rect, origin_spatial_id)?,
                    ));
                }
                clip_chain.entries.push(MpPrimitiveClipEntry::RoundedRect {
                    rect: *rect,
                    radius: radius_to_vec4(*radius),
                    mask_local_from_origin,
                });
            }
            MpClipKind::ImageMask { rect } => {
                if local_fast_path {
                    clip_chain.origin_clip_rect = Some(intersect_rects(
                        clip_chain.origin_clip_rect,
                        clip_rect_in_origin_space(scene, clip.spatial_id, *rect, origin_spatial_id)?,
                    ));
                }
                clip_chain.entries.push(MpPrimitiveClipEntry::ImageMask {
                    rect: *rect,
                    mask_local_from_origin,
                });
            }
            MpClipKind::PlaneSet { planes } => {
                clip_chain.entries.push(MpPrimitiveClipEntry::PlaneSet {
                    planes: transform_planes_to_origin(planes, clip_rect_transform),
                });
            }
        }
    }
    Ok(clip_chain)
}

pub(super) fn clip_rect_in_origin_space(
    scene: &MpScene,
    clip_spatial_id: crate::MpSpatialId,
    rect: Rect,
    origin_spatial_id: Option<crate::MpSpatialId>,
) -> Result<Rect, MpRenderError> {
    scene
        .resolve_rect_in_origin_space(clip_spatial_id, rect, origin_spatial_id)
        .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))
}

pub(super) fn rect_as_plane_set(rect: Rect) -> Vec<Vec4f> {
    vec![
        vec4(1.0, 0.0, 0.0, -(rect.pos.x as f32)),
        vec4(-1.0, 0.0, 0.0, (rect.pos.x + rect.size.x) as f32),
        vec4(0.0, 1.0, 0.0, -(rect.pos.y as f32)),
        vec4(0.0, -1.0, 0.0, (rect.pos.y + rect.size.y) as f32),
    ]
}

/// Transform planes from clip-local space to origin space using the
/// `clip_local_to_origin` matrix.
pub(super) fn transform_planes_to_origin(
    planes: &[Vec4f],
    clip_local_to_origin: Mat4f,
) -> Vec<Vec4f> {
    let plane_transform = clip_local_to_origin.invert().transpose();
    planes
        .iter()
        .map(|plane| plane_transform.transform_vec4(*plane))
        .collect()
}

#[cfg(test)]
mod tests {
    use makepad_widgets::{dvec2, Rect};

    use crate::{MpClipChain, MpClipKind, MpClipNode, MpSceneId};

    use super::*;

    #[test]
    fn lower_clip_chain_keeps_multiple_mask_entries() {
        let mut scene = MpScene::new(
            MpSceneId(3),
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(400.0, 300.0),
            },
        );
        let first = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::RoundedRect {
                rect: Rect {
                    pos: dvec2(20.0, 20.0),
                    size: dvec2(100.0, 80.0),
                },
                radius: crate::MpPerCornerRadius::uniform(12.0),
            },
        });
        let first_chain = scene.push_clip_chain(MpClipChain {
            parent: Some(scene.root_clip_chain_id),
            clips: vec![first],
        });
        let second = scene.push_clip(MpClipNode {
            spatial_id: scene.root_spatial_id,
            kind: MpClipKind::ImageMask {
                rect: Rect {
                    pos: dvec2(30.0, 25.0),
                    size: dvec2(80.0, 60.0),
                },
            },
        });
        let chain = scene.push_clip_chain(MpClipChain {
            parent: Some(first_chain),
            clips: vec![second],
        });

        let lowered = lower_clip_chain(&scene, scene.root_spatial_id, chain, None).unwrap();

        assert_eq!(lowered.entries.len(), 2);
    }
}
