use makepad_widgets::{dvec2, vec4, Rect, Vec4f};

use crate::{embed::MpEmbed, primitive::MpPrimitive, scene::MpScene};

use super::MpRenderError;

pub(super) fn resolve_primitive_rect(
    scene: &MpScene,
    primitive: &MpPrimitive,
    origin_spatial_id: Option<crate::MpSpatialId>,
) -> Result<Rect, MpRenderError> {
    scene
        .resolve_rect_in_origin_space(primitive.spatial_id, primitive.bounds, origin_spatial_id)
        .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))
}

pub(super) fn resolve_embed_rect(
    scene: &MpScene,
    embed: &MpEmbed,
    origin_spatial_id: Option<crate::MpSpatialId>,
) -> Result<Rect, MpRenderError> {
    scene
        .resolve_rect_in_origin_space(embed.spatial_id, embed.bounds, origin_spatial_id)
        .map_err(|err| MpRenderError::UnsupportedSpatial(err.to_string()))
}

pub(super) fn radius_to_vec4(radius: crate::MpPerCornerRadius) -> Vec4f {
    vec4(radius.tl, radius.tr, radius.br, radius.bl)
}

pub(super) fn outset_rect(rect: Rect, delta: f64) -> Rect {
    if delta <= 0.0 {
        return rect;
    }
    Rect {
        pos: dvec2(rect.pos.x - delta, rect.pos.y - delta),
        size: dvec2(rect.size.x + 2.0 * delta, rect.size.y + 2.0 * delta),
    }
}

pub(super) fn intersect_rects(current: Option<Rect>, next: Rect) -> Rect {
    match current {
        None => next,
        Some(current) => {
            let min_x = current.pos.x.max(next.pos.x);
            let min_y = current.pos.y.max(next.pos.y);
            let max_x = (current.pos.x + current.size.x).min(next.pos.x + next.size.x);
            let max_y = (current.pos.y + current.size.y).min(next.pos.y + next.size.y);
            Rect {
                pos: dvec2(min_x, min_y),
                size: dvec2((max_x - min_x).max(0.0), (max_y - min_y).max(0.0)),
            }
        }
    }
}

pub(super) fn union_rects(current: Option<Rect>, next: Rect) -> Option<Rect> {
    match current {
        None => Some(next),
        Some(current) => {
            let min_x = current.pos.x.min(next.pos.x);
            let min_y = current.pos.y.min(next.pos.y);
            let max_x = (current.pos.x + current.size.x).max(next.pos.x + next.size.x);
            let max_y = (current.pos.y + current.size.y).max(next.pos.y + next.size.y);
            Some(Rect {
                pos: dvec2(min_x, min_y),
                size: dvec2(max_x - min_x, max_y - min_y),
            })
        }
    }
}
