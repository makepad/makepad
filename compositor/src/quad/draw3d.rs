use super::clip::{set_clip_masks, set_clip_planes};
use super::types::{MpInternal3dBatch, MpInternal3dQuad};
use super::MpCompositor;
use crate::*;

impl MpCompositor {
    pub(super) fn draw_internal_3d_batch(&mut self, cx: &mut Cx3d, batch: &MpInternal3dBatch) {
        let cx = &mut Cx2d::new(cx.cx);
        for island in &batch.islands {
            for quad in &island.quads {
                if !quad.backface_visible && is_model_backface_culled(quad) {
                    continue;
                }
                self.draw_projective_quad.opacity = quad.opacity.clamp(0.0, 1.0);
                self.draw_projective_quad.draw_super.draw_vars.options.depth_write = quad.depth_write;
                set_clip_planes(cx, &mut self.draw_projective_quad.draw_super.draw_vars, &quad.clip_planes);
                set_clip_masks(cx, &mut self.draw_projective_quad.draw_super.draw_vars, &quad.mask);
                self.draw_projective_quad.set_texture(Some(quad.texture.clone()));
                self.draw_projective_quad
                    .set_matrices(quad.transform_matrix, quad.perspective_matrix);
                self.draw_projective_quad.draw_abs(cx, quad.rect);
            }
        }
    }
}

pub(crate) fn is_model_backface_culled(quad: &MpInternal3dQuad) -> bool {
    let mvp = Mat4f::mul(&quad.perspective_matrix, &quad.transform_matrix);
    let Some(p0) = project_model_point(&mvp, quad.rect.pos, 0.0, 0.0) else {
        return false;
    };
    let Some(p1) = project_model_point(&mvp, quad.rect.pos, quad.rect.size.x as f32, 0.0) else {
        return false;
    };
    let Some(p2) = project_model_point(&mvp, quad.rect.pos, 0.0, quad.rect.size.y as f32) else {
        return false;
    };
    let area = (p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x);
    area < 0.0
}

fn project_model_point(mvp: &Mat4f, rect_pos: DVec2, x: f32, y: f32) -> Option<Vec2f> {
    let clip = mvp.transform_vec4(vec4f(x, y, 0.0, 1.0));
    if clip.w.abs() <= 1e-6 {
        return None;
    }
    Some(vec2(
        rect_pos.x as f32 + clip.x / clip.w,
        rect_pos.y as f32 + clip.y / clip.w,
    ))
}
